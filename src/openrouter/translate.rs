use anyhow::{Context, Result};
use serde_json::json;

use super::client::OpenRouterClient;
use crate::models::Segment;

const TRANSLATION_MODEL: &str = "openai/gpt-4.1";

/// Traduz todos os segmentos para português usando GPT-4.1-mini.
/// Envia em lotes para manter contexto e melhorar qualidade.
pub async fn translate_segments(
    client: &OpenRouterClient,
    segments: &[Segment],
) -> Result<Vec<Segment>> {
    if segments.is_empty() {
        return Ok(Vec::new());
    }

    // Enviar em lotes de ~30 segmentos para balancear contexto e limites de tokens
    let batch_size = 30;
    let mut translated_segments = Vec::new();

    for batch in segments.chunks(batch_size) {
        let batch_result = translate_batch(client, batch).await?;
        translated_segments.extend(batch_result);
    }

    Ok(translated_segments)
}

/// Traduz um lote de segmentos
async fn translate_batch(
    client: &OpenRouterClient,
    segments: &[Segment],
) -> Result<Vec<Segment>> {
    // Montar o JSON dos segmentos com duração para contexto de timing
    let segments_json: Vec<serde_json::Value> = segments
        .iter()
        .map(|s| {
            let duration_secs = (s.end_ms - s.start_ms) as f64 / 1000.0;
            json!({
                "start_ms": s.start_ms,
                "end_ms": s.end_ms,
                "duration_secs": format!("{:.1}", duration_secs),
                "text": s.text
            })
        })
        .collect();

    let prompt = format!(
        r#"You are a professional dubbing translator specializing in Brazilian Portuguese (PT-BR) for video dubbing / voice-over.

Your translation will be read aloud by a TTS engine and MUST fit within the original segment's time window (duration_secs).

CRITICAL RULES:
1. CONCISENESS: The translated text must be approximately the same spoken length as the original. Portuguese tends to be longer than English — compensate by using shorter synonyms, contractions, and natural abbreviations. If the original is 2 seconds, your translation should also take ~2 seconds to speak.
2. NATURAL SPEECH: Translate for SPOKEN Brazilian Portuguese, not written. Use colloquial expressions, contractions ("tá", "né", "pra", "pro", "num", "tô"), and natural speech patterns that a Brazilian would actually say.
3. PRESERVE: Keep proper nouns, brand names, technical terms, and acronyms unchanged (e.g., "GitHub", "API", "React").
4. CONTEXT: Consider the full conversation context. Maintain consistent terminology and register throughout.
5. TIMING: Keep the EXACT same start_ms and end_ms values — do NOT modify timestamps.
6. EMOTION: Preserve the emotional tone — if the original is excited, sarcastic, or calm, the translation should reflect that.

Input segments:
{}

Return ONLY valid JSON, no markdown, no explanation, in this exact format:
{{"segments": [{{"start_ms": 0, "end_ms": 1500, "text": "original text", "translated": "texto traduzido"}}]}}"#,
        serde_json::to_string_pretty(&segments_json)?
    );

    let body = json!({
        "model": TRANSLATION_MODEL,
        "messages": [
            {
                "role": "user",
                "content": prompt
            }
        ],
        "temperature": 0.4,
        "response_format": { "type": "json_object" }
    });

    let response = client.chat_completion(&body).await?;

    let content = response["choices"][0]["message"]["content"]
        .as_str()
        .context("No content in translation response")?;

    let parsed: serde_json::Value =
        serde_json::from_str(content).context("Failed to parse translation JSON")?;

    let translated_array = parsed["segments"]
        .as_array()
        .context("No 'segments' array in translation response")?;

    let mut result: Vec<Segment> = Vec::new();

    for (i, trans) in translated_array.iter().enumerate() {
        let original_segment = segments.get(i);

        let start_ms = trans["start_ms"]
            .as_u64()
            .or_else(|| original_segment.map(|s| s.start_ms))
            .unwrap_or(0);
        let end_ms = trans["end_ms"]
            .as_u64()
            .or_else(|| original_segment.map(|s| s.end_ms))
            .unwrap_or(0);
        let text = trans["text"]
            .as_str()
            .or_else(|| original_segment.map(|s| s.text.as_str()))
            .unwrap_or("")
            .to_string();
        let translated = trans["translated"]
            .as_str()
            .unwrap_or("")
            .to_string();

        result.push(Segment {
            start_ms,
            end_ms,
            text,
            translated_text: translated,
        });
    }

    Ok(result)
}
