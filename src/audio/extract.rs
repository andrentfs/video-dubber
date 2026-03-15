use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Verifica se o ffmpeg está instalado
pub fn check_ffmpeg() -> Result<()> {
    Command::new("ffmpeg")
        .arg("-version")
        .output()
        .context("ffmpeg not found. Please install it: brew install ffmpeg")?;
    Ok(())
}

/// Extrai áudio de um MP4 para WAV (16kHz, mono) — formato ideal para transcrição
pub fn extract_audio(input_mp4: &Path, output_wav: &Path) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args([
            "-y",                          // overwrite output
            "-i", input_mp4.to_str().unwrap(),
            "-vn",                         // no video
            "-ar", "16000",                // 16kHz sample rate
            "-ac", "1",                    // mono
            "-f", "wav",                   // WAV format
            output_wav.to_str().unwrap(),
        ])
        .output()
        .context("Failed to run ffmpeg for audio extraction")?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("ffmpeg audio extraction failed:\n{}", stderr);
    }

    Ok(())
}

/// Obtém a duração de um arquivo de áudio em segundos via ffprobe
pub fn get_audio_duration_secs(audio_path: &Path) -> Result<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v", "quiet",
            "-show_entries", "format=duration",
            "-of", "csv=p=0",
            audio_path.to_str().unwrap(),
        ])
        .output()
        .context("Failed to run ffprobe")?;

    if !output.status.success() {
        anyhow::bail!("ffprobe failed for {:?}", audio_path);
    }

    let duration_str = String::from_utf8_lossy(&output.stdout);
    let duration: f64 = duration_str
        .trim()
        .parse()
        .context("Failed to parse audio duration")?;

    Ok(duration)
}

/// Divide um arquivo WAV em chunks com overlap entre chunks adjacentes.
/// O stride (avanço) é `chunk_duration_secs - overlap_secs`.
/// Retorna tuplas de (chunk_path, offset_ms) onde offset_ms é o timestamp absoluto do início do chunk.
pub fn split_audio_into_chunks(
    wav_path: &Path,
    chunk_duration_secs: u64,
    overlap_secs: u64,
    output_dir: &Path,
) -> Result<Vec<(std::path::PathBuf, u64)>> {
    let total_duration = get_audio_duration_secs(wav_path)?;
    let mut chunks = Vec::new();
    let mut start_secs: u64 = 0;
    let mut chunk_index = 0;
    let stride = chunk_duration_secs.saturating_sub(overlap_secs).max(1);

    while (start_secs as f64) < total_duration {
        let chunk_path = output_dir.join(format!("chunk_{:04}.wav", chunk_index));

        let status = Command::new("ffmpeg")
            .args([
                "-y",
                "-i", wav_path.to_str().unwrap(),
                "-ss", &start_secs.to_string(),
                "-t", &chunk_duration_secs.to_string(),
                "-ar", "16000",
                "-ac", "1",
                "-f", "wav",
                chunk_path.to_str().unwrap(),
            ])
            .output()
            .context("Failed to split audio chunk")?;

        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            anyhow::bail!("ffmpeg chunk split failed:\n{}", stderr);
        }

        let offset_ms = start_secs * 1000;
        chunks.push((chunk_path, offset_ms));
        start_secs += stride;
        chunk_index += 1;
    }

    Ok(chunks)
}

/// Velocidade máxima de aceleração — acima de 1.8x a fala degrada significativamente
const MAX_SPEED_FACTOR: f64 = 1.8;

/// Ajusta a velocidade de um áudio para atingir a duração alvo.
/// Usa apad+atrim para forçar duração exata e evitar drift cumulativo.
#[allow(dead_code)]
pub fn adjust_audio_speed(
    input_path: &Path,
    target_duration_secs: f64,
    output_path: &Path,
) -> Result<()> {
    adjust_audio_speed_inner(input_path, target_duration_secs, output_path)
}

/// Ajusta velocidade com possibilidade de emprestar silêncio adjacente.
/// Se a aceleração necessária excede MAX_SPEED_FACTOR, estende a duração alvo
/// usando até metade do gap de silêncio após o segmento.
/// Retorna a duração efetiva usada.
pub fn adjust_audio_speed_with_overflow(
    input_path: &Path,
    target_duration_secs: f64,
    gap_after_secs: f64,
    output_path: &Path,
) -> Result<f64> {
    let actual_duration = get_audio_duration_secs(input_path)?;

    if actual_duration <= 0.0 || target_duration_secs <= 0.0 {
        std::fs::copy(input_path, output_path)?;
        return Ok(target_duration_secs);
    }

    let speed_factor = actual_duration / target_duration_secs;

    let effective_target = if speed_factor > MAX_SPEED_FACTOR {
        // Calcular quanto tempo precisamos no mínimo para manter MAX_SPEED_FACTOR
        let needed_duration = actual_duration / MAX_SPEED_FACTOR;
        let overflow = needed_duration - target_duration_secs;
        let borrowable = gap_after_secs * 0.5; // emprestar no máximo metade do gap
        let extended = target_duration_secs + overflow.min(borrowable);
        if extended > target_duration_secs {
            eprintln!(
                "   ↪ Segmento {:.2}s emprestou {:.2}s de silêncio adjacente (gap: {:.2}s)",
                target_duration_secs,
                extended - target_duration_secs,
                gap_after_secs
            );
        }
        extended
    } else {
        target_duration_secs
    };

    adjust_audio_speed_inner(input_path, effective_target, output_path)?;
    Ok(effective_target)
}

/// Implementação interna de ajuste de velocidade
fn adjust_audio_speed_inner(
    input_path: &Path,
    target_duration_secs: f64,
    output_path: &Path,
) -> Result<()> {
    let actual_duration = get_audio_duration_secs(input_path)?;

    if actual_duration <= 0.0 || target_duration_secs <= 0.0 {
        std::fs::copy(input_path, output_path)?;
        return Ok(());
    }

    let speed_factor = actual_duration / target_duration_secs;

    let clamped_factor = speed_factor.clamp(0.5, MAX_SPEED_FACTOR);

    if (speed_factor - clamped_factor).abs() > 0.01 {
        eprintln!(
            "⚠️  Speed factor extremo: {:.2}x (clampado para {:.2}x) — segmento {:.2}s → alvo {:.2}s",
            speed_factor, clamped_factor, actual_duration, target_duration_secs
        );
    }

    let atempo_filters = build_atempo_chain(clamped_factor);

    // apad preenche com silêncio se áudio ficou curto, atrim corta se ficou longo
    // Isso garante duração exata e elimina drift de arredondamento
    let af_filter = format!(
        "{},apad=whole_dur={:.4},atrim=0:{:.4}",
        atempo_filters, target_duration_secs, target_duration_secs
    );

    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-i", input_path.to_str().unwrap(),
            "-af", &af_filter,
            "-ar", "24000",
            "-ac", "1",
            output_path.to_str().unwrap(),
        ])
        .output()
        .context("Failed to adjust audio speed")?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("ffmpeg speed adjustment failed:\n{}", stderr);
    }

    Ok(())
}

/// Constrói cadeia de filtros atempo para fatores no range [0.5, 2.0]
fn build_atempo_chain(mut factor: f64) -> String {
    factor = factor.clamp(0.5, 2.0);

    let mut filters = Vec::new();

    while factor < 0.5 {
        filters.push("atempo=0.5".to_string());
        factor /= 0.5;
    }

    filters.push(format!("atempo={:.4}", factor));
    filters.join(",")
}

/// Substitui o áudio de um vídeo pelo áudio dublado.
/// Mantém o vídeo original intacto, apenas troca a faixa de áudio.
pub fn merge_audio_into_video(
    original_video: &Path,
    dubbed_audio: &Path,
    output_video: &Path,
) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-i", original_video.to_str().unwrap(),
            "-i", dubbed_audio.to_str().unwrap(),
            "-c:v", "copy",          // copiar vídeo sem re-encodar
            "-map", "0:v:0",         // vídeo do primeiro input
            "-map", "1:a:0",         // áudio do segundo input
            "-shortest",             // encerrar no stream mais curto
            output_video.to_str().unwrap(),
        ])
        .output()
        .context("Failed to merge audio into video")?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("ffmpeg merge audio+video failed:\n{}", stderr);
    }

    Ok(())
}
