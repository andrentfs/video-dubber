use anyhow::{Context, Result};
use base64::Engine;
use serde_json::json;
use std::path::Path;
use std::process::Command;

use super::client::OpenRouterClient;

const TTS_MODEL: &str = "openai/gpt-audio-mini";

/// Tamanho mínimo esperado de PCM por segundo de fala (24kHz, 16bit, mono = 48000 bytes/s)
/// Usamos 20% como threshold mínimo para detectar áudio truncado
const MIN_BYTES_PER_SEC: usize = 48000 / 5; // ~9600 bytes/s

/// Gera áudio TTS usando openai/gpt-audio-mini via OpenRouter.
/// Usa streaming incremental real para evitar perda de dados em conexões instáveis.
pub async fn generate_speech(
    client: &OpenRouterClient,
    text: &str,
    voice: &str,
) -> Result<Vec<u8>> {
    let body = json!({
        "model": TTS_MODEL,
        "modalities": ["text", "audio"],
        "stream": true,
        "audio": {
            "voice": voice,
            "format": "pcm16"
        },
        "messages": [
            {
                "role": "system",
                "content": "You are a professional Brazilian Portuguese voice actor performing a dubbing. Read the text exactly as written with natural prosody, appropriate emotion, and conversational pacing. Do NOT add any words, greetings, commentary, or responses to the content — speak ONLY the provided text. Match the emotional tone implied by the text (excited, calm, serious, etc.)."
            },
            {
                "role": "user",
                "content": format!("Leia o seguinte texto em voz alta exatamente como escrito, com entonação natural:\n\n{}", text)
            }
        ],
        "temperature": 0.3
    });

    let url = format!("{}/chat/completions", client.base_url);

    let response = client
        .client
        .post(&url)
        .json(&body)
        .send()
        .await
        .context("Failed to send TTS streaming request")?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "OpenRouter TTS API error ({}): {}",
            status,
            &error_text[..error_text.len().min(500)]
        );
    }

    // Streaming incremental: ler o body em chunks conforme chegam
    let mut pcm_bytes: Vec<u8> = Vec::new();
    let mut buffer = String::new();
    let mut stream_errors: usize = 0;

    let mut byte_stream = response.bytes_stream();
    use futures::StreamExt;

    while let Some(chunk_result) = byte_stream.next().await {
        let chunk = chunk_result.context("Stream interrupted while receiving TTS audio")?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Processar linhas completas do buffer
        while let Some(newline_pos) = buffer.find('\n') {
            let line = buffer[..newline_pos].trim().to_string();
            buffer = buffer[newline_pos + 1..].to_string();

            if !line.starts_with("data: ") {
                continue;
            }
            let data = &line[6..];
            if data == "[DONE]" {
                break;
            }

            match serde_json::from_str::<serde_json::Value>(data) {
                Ok(chunk_json) => {
                    if let Some(audio_data) =
                        chunk_json["choices"][0]["delta"]["audio"]["data"].as_str()
                    {
                        if !audio_data.is_empty() {
                            match base64::engine::general_purpose::STANDARD.decode(audio_data) {
                                Ok(bytes) => pcm_bytes.extend_from_slice(&bytes),
                                Err(_) => stream_errors += 1,
                            }
                        }
                    }
                }
                Err(_) => {
                    stream_errors += 1;
                }
            }
        }
    }

    if pcm_bytes.is_empty() {
        anyhow::bail!(
            "TTS returned no audio data for text: \"{}\"",
            &text[..text.len().min(60)]
        );
    }

    // Validar tamanho mínimo do áudio baseado no comprimento do texto
    // Heurística: ~5 caracteres por segundo de fala em português
    let estimated_duration_secs = (text.len() as f64 / 12.0).max(0.5);
    let min_expected_bytes = (estimated_duration_secs * MIN_BYTES_PER_SEC as f64) as usize;

    if pcm_bytes.len() < min_expected_bytes {
        anyhow::bail!(
            "TTS audio seems truncated: got {} bytes, expected at least {} for text: \"{}\"",
            pcm_bytes.len(),
            min_expected_bytes,
            &text[..text.len().min(60)]
        );
    }

    if stream_errors > 0 {
        eprintln!(
            "⚠️  {} chunk(s) SSE com erro de parse foram ignorados",
            stream_errors
        );
    }

    Ok(pcm_bytes)
}

/// Gera áudio TTS, converte PCM16 → WAV, e salva no caminho especificado
pub async fn generate_speech_to_file(
    client: &OpenRouterClient,
    text: &str,
    voice: &str,
    output_path: &Path,
) -> Result<()> {
    let pcm_bytes = generate_speech(client, text, voice).await?;

    // Salvar PCM16 raw em arquivo temporário
    let pcm_path = output_path.with_extension("pcm");
    std::fs::write(&pcm_path, &pcm_bytes).context("Failed to write PCM audio")?;

    // Converter PCM16 → WAV com ffmpeg (24kHz mono, formato padrão do OpenAI TTS)
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "s16le",
            "-ar",
            "24000",
            "-ac",
            "1",
            "-i",
            pcm_path.to_str().unwrap(),
            output_path.to_str().unwrap(),
        ])
        .output()
        .context("Failed to convert PCM to WAV")?;

    // Limpar arquivo PCM temporário
    std::fs::remove_file(&pcm_path).ok();

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("ffmpeg PCM→WAV conversion failed:\n{}", stderr);
    }

    Ok(())
}
