use anyhow::{Context, Result};
use base64::Engine;
use serde_json::json;
use std::path::Path;

use super::client::OpenRouterClient;
use crate::models::Segment;

const TRANSCRIPTION_MODEL: &str = "google/gemini-2.5-flash";

/// Transcreve um chunk de áudio usando Gemini 2.5 Flash via OpenRouter.
/// Retorna segmentos com timestamps (start_ms, end_ms, text).
/// O offset_ms é adicionado aos timestamps para chunks que não começam no zero.
/// chunk_duration_hint_secs dá ao modelo uma âncora temporal para melhor precisão.
pub async fn transcribe_audio_chunk(
    client: &OpenRouterClient,
    wav_path: &Path,
    offset_ms: u64,
    chunk_duration_hint_secs: Option<f64>,
) -> Result<Vec<Segment>> {
    // Ler e encodar áudio em base64
    let audio_bytes = std::fs::read(wav_path)
        .context(format!("Failed to read audio file: {:?}", wav_path))?;
    let audio_base64 = base64::engine::general_purpose::STANDARD.encode(&audio_bytes);

    let duration_hint = match chunk_duration_hint_secs {
        Some(d) => format!("\nThis audio clip is {:.1} seconds long. Use this as a reference — your last segment's end_ms should be close to {:.0}ms if speech continues to the end.\n", d, d * 1000.0),
        None => String::new(),
    };

    let prompt = format!(
        r#"You are a precise audio transcription engine for video dubbing. Transcribe ALL speech in the following audio with accurate timestamps.
{duration_hint}
CRITICAL RULES:
- You MUST transcribe the ENTIRE audio from beginning to end — every single word
- Do NOT skip any speech, even if it is quiet, fast, or overlapping
- Timestamps must be in milliseconds relative to the start of this audio clip
- Each segment should be a natural sentence or phrase (typically 3-8 seconds). Prefer complete sentences over fragments
- Do NOT create segments shorter than 1 second unless it's a single isolated word
- Transcribe in the original language of the audio — do NOT translate
- Be precise with start_ms and end_ms: start when the speaker begins, end when they stop (include trailing words, don't cut mid-word)
- Minimize gaps between consecutive speech segments — if speech is continuous, segments should be nearly adjacent
- If there are multiple speakers, still transcribe everything in chronological order
- Include filler words and interjections only if they are meaningful ("uh", "hmm" can be omitted)
- If there is no speech at all, return {{"segments": []}}

Return ONLY valid JSON in this exact format, no markdown, no explanation, no code blocks:

{{"segments": [{{"start_ms": 0, "end_ms": 3500, "text": "first sentence here"}}, {{"start_ms": 3600, "end_ms": 7200, "text": "second sentence here"}}]}}"#);

    let body = json!({
        "model": TRANSCRIPTION_MODEL,
        "messages": [
            {
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": prompt
                    },
                    {
                        "type": "input_audio",
                        "input_audio": {
                            "data": audio_base64,
                            "format": "wav"
                        }
                    }
                ]
            }
        ],
        "temperature": 0.1,
        "response_format": { "type": "json_object" }
    });

    let response = client.chat_completion(&body).await?;

    // Extrair conteúdo da resposta
    let content = response["choices"][0]["message"]["content"]
        .as_str()
        .context("No content in transcription response")?;

    // Parsear JSON
    let parsed: serde_json::Value =
        serde_json::from_str(content).context("Failed to parse transcription JSON")?;

    let segments_array = parsed["segments"]
        .as_array()
        .context("No 'segments' array in response")?;

    let mut segments: Vec<Segment> = Vec::new();
    for seg in segments_array {
        let start_ms = seg["start_ms"].as_u64().unwrap_or(0) + offset_ms;
        let end_ms = seg["end_ms"].as_u64().unwrap_or(0) + offset_ms;
        let text = seg["text"].as_str().unwrap_or("").to_string();

        if !text.is_empty() && end_ms > start_ms {
            segments.push(Segment {
                start_ms,
                end_ms,
                text,
                translated_text: String::new(),
                effective_duration_secs: None,
            });
        }
    }

    Ok(segments)
}
