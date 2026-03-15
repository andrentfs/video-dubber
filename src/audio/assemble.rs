use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

use crate::models::Segment;

/// Tamanho máximo de batch para o filter_complex do ffmpeg (limite prático de inputs)
const BATCH_SIZE: usize = 40;

/// Monta o áudio final posicionando cada segmento em seu timestamp absoluto.
/// Usa adelay+amix do ffmpeg para que cada segmento seja independente,
/// eliminando o drift cumulativo do concat demuxer anterior.
pub fn assemble_segments(
    synced_segment_paths: &[(usize, &Path)],
    segments: &[Segment],
    total_duration_secs: f64,
    output_path: &Path,
    temp_dir: &Path,
) -> Result<()> {
    if synced_segment_paths.is_empty() {
        anyhow::bail!("No audio segments to assemble");
    }

    // Gerar track de silêncio com a duração total do vídeo (base para mixagem)
    let silence_base = temp_dir.join("silence_base.wav");
    generate_silence(&silence_base, total_duration_secs)?;

    // Processar em batches para não exceder limite de inputs do ffmpeg
    let batches: Vec<&[(usize, &Path)]> = synced_segment_paths.chunks(BATCH_SIZE).collect();

    if batches.len() == 1 {
        // Batch único — mixar direto no output
        mix_batch_with_adelay(
            &silence_base,
            batches[0],
            segments,
            output_path,
            temp_dir,
            "batch0",
        )?;
    } else {
        // Múltiplos batches — gerar intermediários e depois mixar
        let mut batch_outputs: Vec<std::path::PathBuf> = Vec::new();

        for (batch_idx, batch) in batches.iter().enumerate() {
            let batch_output = temp_dir.join(format!("batch_{:04}.wav", batch_idx));
            mix_batch_with_adelay(
                &silence_base,
                batch,
                segments,
                &batch_output,
                temp_dir,
                &format!("batch{}", batch_idx),
            )?;
            batch_outputs.push(batch_output);
        }

        // Merge final: mixar todos os batches
        merge_batch_outputs(&batch_outputs, output_path)?;
    }

    Ok(())
}

/// Mixa um batch de segmentos sobre a base de silêncio usando adelay+amix.
/// Cada segmento é posicionado em seu start_ms absoluto.
fn mix_batch_with_adelay(
    silence_base: &Path,
    batch: &[(usize, &Path)],
    segments: &[Segment],
    output_path: &Path,
    _temp_dir: &Path,
    _batch_name: &str,
) -> Result<()> {
    if batch.is_empty() {
        std::fs::copy(silence_base, output_path)?;
        return Ok(());
    }

    // Construir argumentos de input: silence + cada segmento do batch
    let mut args: Vec<String> = vec![
        "-y".to_string(),
        "-i".to_string(),
        silence_base.to_str().unwrap().to_string(),
    ];

    for &(_seg_idx, seg_path) in batch {
        args.push("-i".to_string());
        args.push(seg_path.to_str().unwrap().to_string());
    }

    // Construir filter_complex:
    // [1]adelay=START_MS|START_MS[s0]; [2]adelay=START_MS|START_MS[s1]; ...
    // [0][s0][s1]...amix=inputs=N:duration=first:normalize=0
    let mut filter_parts: Vec<String> = Vec::new();
    let mut mix_inputs = "[0]".to_string(); // silence base

    for (i, &(seg_idx, _seg_path)) in batch.iter().enumerate() {
        let input_idx = i + 1; // +1 porque [0] é o silence
        let start_ms = segments[seg_idx].start_ms;
        let label = format!("s{}", i);

        // Aplicar fade-in (10ms) e fade-out (15ms) para evitar cliques/pops nos cortes
        let seg_duration_ms = segments[seg_idx].end_ms.saturating_sub(segments[seg_idx].start_ms);
        let fade_out_start_ms = seg_duration_ms.saturating_sub(15);
        filter_parts.push(format!(
            "[{}]afade=t=in:st=0:d=0.010,afade=t=out:st={:.3}:d=0.015,adelay={}|{}[{}]",
            input_idx,
            fade_out_start_ms as f64 / 1000.0,
            start_ms,
            start_ms,
            label
        ));
        mix_inputs.push_str(&format!("[{}]", label));
    }

    let total_inputs = batch.len() + 1; // silence + segments
    // amix sem normalização para preservar níveis individuais,
    // seguido de dynaudnorm para normalização suave de volume
    filter_parts.push(format!(
        "{}amix=inputs={}:duration=first:normalize=0,dynaudnorm=f=150:g=15:p=0.95:m=10",
        mix_inputs, total_inputs
    ));

    let filter_complex = filter_parts.join(";");

    args.extend_from_slice(&[
        "-filter_complex".to_string(),
        filter_complex,
        "-ar".to_string(),
        "24000".to_string(),
        "-ac".to_string(),
        "1".to_string(),
        output_path.to_str().unwrap().to_string(),
    ]);

    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let status = Command::new("ffmpeg")
        .args(&args_ref)
        .output()
        .context("Failed to run ffmpeg adelay+amix")?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("ffmpeg adelay+amix failed:\n{}", stderr);
    }

    Ok(())
}

/// Mixa múltiplos batch outputs em um único arquivo final usando amix.
fn merge_batch_outputs(batch_paths: &[std::path::PathBuf], output_path: &Path) -> Result<()> {
    if batch_paths.len() == 1 {
        std::fs::copy(&batch_paths[0], output_path)?;
        return Ok(());
    }

    let mut args: Vec<String> = vec!["-y".to_string()];

    for path in batch_paths {
        args.push("-i".to_string());
        args.push(path.to_str().unwrap().to_string());
    }

    // Construir filter: [0][1][2]...amix=inputs=N:duration=longest:normalize=0
    let mut mix_inputs = String::new();
    for i in 0..batch_paths.len() {
        mix_inputs.push_str(&format!("[{}]", i));
    }

    let filter = format!(
        "{}amix=inputs={}:duration=longest:normalize=0",
        mix_inputs,
        batch_paths.len()
    );

    args.extend_from_slice(&[
        "-filter_complex".to_string(),
        filter,
        "-ar".to_string(),
        "24000".to_string(),
        "-ac".to_string(),
        "1".to_string(),
        output_path.to_str().unwrap().to_string(),
    ]);

    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let status = Command::new("ffmpeg")
        .args(&args_ref)
        .output()
        .context("Failed to merge batch outputs")?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("ffmpeg batch merge failed:\n{}", stderr);
    }

    Ok(())
}

/// Gera um arquivo WAV de silêncio com a duração especificada (24kHz mono)
fn generate_silence(output_path: &Path, duration_secs: f64) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f", "lavfi",
            "-i", &format!("anullsrc=r=24000:cl=mono,atrim=0:{:.4}", duration_secs),
            "-ar", "24000",
            "-ac", "1",
            output_path.to_str().unwrap(),
        ])
        .output()
        .context("Failed to generate silence")?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("ffmpeg silence generation failed:\n{}", stderr);
    }

    Ok(())
}
