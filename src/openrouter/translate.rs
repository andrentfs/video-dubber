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
    // Montar o JSON dos segmentos com duração e max_chars para contexto de timing
    let segments_json: Vec<serde_json::Value> = segments
        .iter()
        .map(|s| {
            let duration_secs = (s.end_ms - s.start_ms) as f64 / 1000.0;
            let max_chars = (duration_secs * 13.0).floor() as usize; // 13 chars/s = ritmo natural PT-BR
            json!({
                "start_ms": s.start_ms,
                "end_ms": s.end_ms,
                "duration_secs": format!("{:.1}", duration_secs),
                "max_chars": max_chars,
                "text": s.text
            })
        })
        .collect();

    let prompt = format!(
        r#"You are a professional dubbing translator specializing in Brazilian Portuguese (PT-BR) for video dubbing / voice-over.

Your translation will be read aloud by a TTS engine and MUST fit within the original segment's time window.

CRITICAL RULES:
1. CHARACTER LIMIT: Your translation for each segment MUST NOT exceed `max_chars` characters (including spaces). This is a HARD LIMIT — if the natural translation exceeds it, use shorter synonyms, contractions, drop filler words, restructure the sentence. Count characters carefully. Example: if max_chars is 26, your translation must be ≤26 characters.
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
        "temperature": 0.3,
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

        let seg_result = Segment {
            start_ms,
            end_ms,
            text,
            translated_text: translated,
            effective_duration_secs: None,
        };

        // Validação pós-tradução: logar segmentos que excedem o limite
        let max_chars = seg_result.max_chars(13.0);
        if max_chars > 0 && seg_result.translated_text.len() > max_chars {
            eprintln!(
                "⚠️  Seg [{:.1}s]: tradução com {} chars (max: {}) → candidato a re-tradução",
                seg_result.duration_secs(),
                seg_result.translated_text.len(),
                max_chars
            );
        }

        result.push(seg_result);
    }

    Ok(result)
}

/// Re-traduz um único segmento com prompt agressivo de brevidade.
/// Retorna o novo texto traduzido ou erro.
async fn retranslate_segment(
    client: &OpenRouterClient,
    segment: &Segment,
    max_chars: usize,
) -> Result<String> {
    let current_len = segment.translated_text.len();

    let prompt = format!(
        r#"You are a Brazilian Portuguese dubbing translator. Your previous translation was too long ({current_len} characters) for a {duration:.1}s segment.

Translate the following text again in AT MOST {max_chars} characters (including spaces). This is a HARD LIMIT.

Prioritize brevity:
- Paraphrase freely — you do NOT need to translate word-for-word
- Use contractions: tá, né, pra, pro, num, tô, dum, duma
- Omit filler words and secondary information if needed
- Keep proper nouns and technical terms unchanged

Original text: "{original}"

Previous translation ({current_len} chars): "{previous}"

Return ONLY the new translated text, nothing else. No quotes, no JSON, no explanation."#,
        current_len = current_len,
        duration = segment.duration_secs(),
        max_chars = max_chars,
        original = segment.text,
        previous = segment.translated_text,
    );

    let body = json!({
        "model": TRANSLATION_MODEL,
        "messages": [
            {
                "role": "user",
                "content": prompt
            }
        ],
        "temperature": 0.2
    });

    let response = client.chat_completion(&body).await?;

    let content = response["choices"][0]["message"]["content"]
        .as_str()
        .context("No content in retranslation response")?
        .trim()
        .to_string();

    Ok(content)
}

/// Valida e corrige traduções que excedem a taxa de fala máxima.
/// Re-traduz segmentos com >14 chars/s usando prompt mais agressivo.
/// Retorna a quantidade de segmentos re-traduzidos.
pub async fn validate_and_fix_translations(
    client: &OpenRouterClient,
    segments: &mut [Segment],
) -> Result<usize> {
    let mut retranslated_count = 0;

    for seg in segments.iter_mut() {
        if seg.translated_chars_per_sec() <= 14.0 {
            continue;
        }

        let max_chars = seg.max_chars(13.0);
        if max_chars == 0 {
            continue;
        }

        // Até 2 tentativas, reduzindo max_chars em 15% a cada retry
        let mut best_text = seg.translated_text.clone();
        let mut current_max = max_chars;

        for attempt in 0..2 {
            match retranslate_segment(client, seg, current_max).await {
                Ok(new_text) => {
                    if new_text.len() < best_text.len() {
                        best_text = new_text;
                    }
                    // Se já cabe, parar
                    if best_text.len() <= max_chars {
                        break;
                    }
                    // Reduzir limite em 15% para próxima tentativa
                    current_max = (current_max as f64 * 0.85).floor() as usize;
                }
                Err(e) => {
                    eprintln!(
                        "⚠️  Re-tradução tentativa {} falhou para seg [{:.1}s]: {}",
                        attempt + 1,
                        seg.duration_secs(),
                        e
                    );
                    break;
                }
            }
        }

        if best_text != seg.translated_text {
            seg.translated_text = best_text;
            retranslated_count += 1;
        }
    }

    Ok(retranslated_count)
}
