use anyhow::{Context, Result};

use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tokio::sync::Semaphore;
use std::sync::Arc;

use crate::audio::{assemble, extract};
use crate::models::{Config, Segment};
use crate::openrouter::{client::OpenRouterClient, transcribe, translate, tts};

const CACHE_TRANSCRIPTION_FILE: &str = "cache_transcription.json";
const CACHE_TRANSLATION_FILE: &str = "cache_translation.json";

/// Duração mínima em ms para um segmento de TTS.
/// Segmentos menores serão agrupados com o seguinte.
const MIN_SEGMENT_DURATION_MS: u64 = 2000;

/// Percentual máximo de falhas permitido antes de abortar o pipeline
const MAX_FAILURE_PERCENT: f64 = 10.0;

/// Executa o pipeline completo de dublagem
pub async fn run(config: &Config) -> Result<()> {
    // Verificar ffmpeg
    extract::check_ffmpeg()?;

    // Criar diretório temporário
    let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
    let temp_path = temp_dir.path();

    // Diretório de cache no mesmo local do output
    let cache_dir = config.output_path.parent().unwrap_or(std::path::Path::new("."));

    // Inicializar cliente OpenRouter
    let client = OpenRouterClient::new(&config.api_key, &config.base_url)?;

    // ═══════════════════════════════════════════
    // ESTÁGIO 1: Extração de áudio
    // ═══════════════════════════════════════════
    let step_bar = create_step_bar("🎵 Extraindo áudio do vídeo...");
    let full_wav = temp_path.join("full_audio.wav");
    extract::extract_audio(&config.input_path, &full_wav)?;
    let total_duration = extract::get_audio_duration_secs(&full_wav)?;
    step_bar.finish_with_message(format!(
        "🎵 Áudio extraído ({:.1}s de duração)",
        total_duration
    ));

    // ═══════════════════════════════════════════
    // ESTÁGIO 2: Dividir em chunks (se necessário)
    // ═══════════════════════════════════════════
    let chunks = if total_duration > config.chunk_duration_secs as f64 {
        let step_bar = create_step_bar("✂️  Dividindo áudio em chunks...");
        let chunk_paths =
            extract::split_audio_into_chunks(&full_wav, config.chunk_duration_secs, temp_path)?;
        step_bar.finish_with_message(format!("✂️  Dividido em {} chunks", chunk_paths.len()));
        chunk_paths
    } else {
        vec![full_wav.clone()]
    };

    // ═══════════════════════════════════════════
    // ESTÁGIO 3: Transcrição (Gemini 2.5 Flash)
    // ═══════════════════════════════════════════
    let segments = {
        let cache_path = cache_dir.join(CACHE_TRANSCRIPTION_FILE);

        if cache_path.exists() {
            println!("📋 Cache de transcrição encontrado, reutilizando...");
            let cached = std::fs::read_to_string(&cache_path)?;
            serde_json::from_str::<Vec<Segment>>(&cached)
                .context("Failed to parse transcription cache")?
        } else {
            let pb = create_progress_bar(chunks.len() as u64, "🎙️  Transcrevendo");

            let mut all_segments = Vec::new();
            for (i, chunk_path) in chunks.iter().enumerate() {
                let offset_ms = i as u64 * config.chunk_duration_secs * 1000;
                // Retry na transcrição: até 2 tentativas adicionais
                let mut chunk_segments = None;
                let mut last_err = String::new();
                for attempt in 0..3 {
                    match transcribe::transcribe_audio_chunk(&client, chunk_path, offset_ms).await {
                        Ok(segs) => {
                            chunk_segments = Some(segs);
                            break;
                        }
                        Err(e) => {
                            last_err = format!("{}", e);
                            if attempt < 2 {
                                eprintln!("⚠️  Transcrição chunk {} tentativa {}/3 falhou: {}", i, attempt + 1, last_err);
                                tokio::time::sleep(tokio::time::Duration::from_secs(2 * (attempt as u64 + 1))).await;
                            }
                        }
                    }
                }
                match chunk_segments {
                    Some(segs) => all_segments.extend(segs),
                    None => anyhow::bail!("Transcrição do chunk {} falhou após 3 tentativas: {}", i, last_err),
                }
                pb.inc(1);
            }

            all_segments.sort_by_key(|s| s.start_ms);

            // Remover segmentos sobrepostos (acontece na junção de chunks)
            all_segments = deduplicate_segments(all_segments);

            pb.finish_with_message(format!(
                "🎙️  Transcritos {} segmentos",
                all_segments.len()
            ));

            // Salvar cache
            let cache_json = serde_json::to_string_pretty(&all_segments)?;
            std::fs::write(&cache_path, cache_json).ok();

            all_segments
        }
    };

    if segments.is_empty() {
        anyhow::bail!("No speech segments were detected in the audio.");
    }

    println!("   → {} segmentos detectados", segments.len());

    // ═══════════════════════════════════════════
    // ESTÁGIO 4: Tradução (GPT-4.1-mini)
    // ═══════════════════════════════════════════
    let translated_segments = {
        let cache_path = cache_dir.join(CACHE_TRANSLATION_FILE);

        if cache_path.exists() {
            println!("📋 Cache de tradução encontrado, reutilizando...");
            let cached = std::fs::read_to_string(&cache_path)?;
            serde_json::from_str::<Vec<Segment>>(&cached)
                .context("Failed to parse translation cache")?
        } else {
            let step_bar = create_step_bar("🌐 Traduzindo para PT-BR...");
            let translated = translate::translate_segments(&client, &segments).await?;
            step_bar.finish_with_message("🌐 Tradução concluída");

            // Salvar cache
            let cache_json = serde_json::to_string_pretty(&translated)?;
            std::fs::write(&cache_path, cache_json).ok();

            translated
        }
    };

    // Exibir amostra da tradução
    for seg in translated_segments.iter().take(3) {
        println!(
            "   [{:.1}s-{:.1}s] \"{}\" → \"{}\"",
            seg.start_ms as f64 / 1000.0,
            seg.end_ms as f64 / 1000.0,
            &seg.text[..seg.text.len().min(40)],
            &seg.translated_text[..seg.translated_text.len().min(40)]
        );
    }
    if translated_segments.len() > 3 {
        println!("   ... e mais {} segmentos", translated_segments.len() - 3);
    }

    // ═══════════════════════════════════════════
    // ESTÁGIO 4.5: Agrupar segmentos curtos
    // ═══════════════════════════════════════════
    let merged_segments = merge_short_segments(&translated_segments);
    if merged_segments.len() < translated_segments.len() {
        println!(
            "   → {} segmentos agrupados (de {} originais) para estabilidade do TTS",
            merged_segments.len(),
            translated_segments.len()
        );
    }

    // Análise de expansão de texto: detectar segmentos onde a tradução é muito mais longa que o original
    {
        let mut extreme_segments = 0;
        for seg in &merged_segments {
            let duration_secs = seg.duration_secs();
            // Heurística: ~4 sílabas/seg em fala normal pt-br ≈ ~12 chars/seg
            let max_chars_for_duration = (duration_secs * 14.0) as usize;
            if seg.translated_text.len() > max_chars_for_duration && duration_secs < 5.0 {
                extreme_segments += 1;
            }
        }
        if extreme_segments > 0 {
            eprintln!(
                "⚠️  {} segmento(s) com tradução possivelmente longa demais para o tempo disponível — o TTS irá acelerar",
                extreme_segments
            );
        }
    }

    // ═══════════════════════════════════════════
    // ESTÁGIO 5: TTS com paralelismo (GPT-4o-mini-tts)
    // ═══════════════════════════════════════════
    let pb = create_progress_bar(merged_segments.len() as u64, "🔊 Gerando áudio");
    let semaphore = Arc::new(Semaphore::new(config.max_concurrent_tts));

    let tts_tasks: Vec<_> = merged_segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            let client = client.clone();
            let voice = config.voice.clone();
            let text = seg.translated_text.clone();
            let raw_path = temp_path.join(format!("tts_raw_{:04}.wav", i));
            let synced_path = temp_path.join(format!("tts_synced_{:04}.wav", i));
            let target_duration = seg.duration_secs();
            let sem = semaphore.clone();
            let pb = pb.clone();

            tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();

                let mut retries = 0;
                let max_retries = 3;
                let mut last_error = String::new();

                while retries < max_retries {
                    match tts::generate_speech_to_file(&client, &text, &voice, &raw_path).await {
                        Ok(_) => {
                            match extract::adjust_audio_speed(&raw_path, target_duration, &synced_path) {
                                Ok(_) => {
                                    pb.inc(1);
                                    return Ok::<(usize, Option<PathBuf>), anyhow::Error>((i, Some(synced_path)));
                                }
                                Err(e) => {
                                    last_error = format!("speed adjust: {}", e);
                                    break; // Erro local no ffmpeg, não adianta retry
                                }
                            }
                        }
                        Err(e) => {
                            retries += 1;
                            last_error = format!("{}", e);
                            eprintln!(
                                "\n⚠️  TTS seg {} tentativa {}/{}: {}",
                                i, retries, max_retries, last_error
                            );
                            if retries < max_retries {
                                let base_ms: u64 = match retries {
                                    1 => 2000,
                                    2 => 4000,
                                    _ => 8000,
                                };
                                // Jitter: ±30% para evitar thundering herd
                                let jitter = (base_ms as f64 * 0.3 * (i as f64 % 7.0 / 7.0)) as u64;
                                let wait_ms = base_ms + jitter;
                                tokio::time::sleep(tokio::time::Duration::from_millis(wait_ms)).await;
                            }
                        }
                    }
                }

                pb.inc(1);
                eprintln!(
                    "❌ Segmento {} falhou após {} tentativas: {}",
                    i, max_retries, last_error
                );
                Ok::<(usize, Option<PathBuf>), anyhow::Error>((i, None))
            })
        })
        .collect();

    // Aguardar todas as tasks de TTS
    let mut synced_paths: Vec<(usize, PathBuf)> = Vec::new();
    let mut failed_count: usize = 0;

    for task in tts_tasks {
        match task.await.context("TTS task panicked")? {
            Ok((idx, Some(path))) => synced_paths.push((idx, path)),
            Ok((_idx, None)) => failed_count += 1,
            Err(e) => {
                eprintln!("❌ Erro crítico em task TTS: {}", e);
                failed_count += 1;
            }
        }
    }

    // Verificar se o percentual de falhas é aceitável
    let total_segments = merged_segments.len();
    let failure_percent = (failed_count as f64 / total_segments as f64) * 100.0;

    if failed_count > 0 {
        eprintln!(
            "\n⚠️  {}/{} segmentos falharam ({:.1}%)",
            failed_count, total_segments, failure_percent
        );
    }

    if failure_percent > MAX_FAILURE_PERCENT {
        anyhow::bail!(
            "Muitos segmentos TTS falharam: {}/{} ({:.1}%). Limite é {:.0}%. \
             Verifique sua conexão e API key, e tente novamente (o cache de transcrição/tradução será reutilizado).",
            failed_count,
            total_segments,
            failure_percent,
            MAX_FAILURE_PERCENT
        );
    }

    // Ordenar por índice do segmento
    synced_paths.sort_by_key(|(idx, _)| *idx);

    pb.finish_with_message(format!(
        "🔊 Áudio gerado ({}/{} segmentos OK)",
        total_segments - failed_count,
        total_segments
    ));

    // Diagnóstico de drift: comparar duração real dos segmentos sincronizados com alvo
    {
        let mut drift_stats: Vec<f64> = Vec::new();
        for &(idx, ref path) in &synced_paths {
            if let Ok(actual) = extract::get_audio_duration_secs(path) {
                let target = merged_segments[idx].duration_secs();
                let drift_ms = (actual - target).abs() * 1000.0;
                drift_stats.push(drift_ms);
            }
        }
        if !drift_stats.is_empty() {
            let avg_drift = drift_stats.iter().sum::<f64>() / drift_stats.len() as f64;
            let max_drift = drift_stats.iter().cloned().fold(0.0_f64, f64::max);
            let over_50ms = drift_stats.iter().filter(|&&d| d > 50.0).count();
            println!(
                "   📊 Drift stats: avg={:.1}ms, max={:.1}ms, >{} com >50ms drift ({} segmentos)",
                avg_drift, max_drift, over_50ms, drift_stats.len()
            );
        }
    }

    // ═══════════════════════════════════════════
    // ESTÁGIO 6: Montagem do áudio dublado (WAV temp)
    // ═══════════════════════════════════════════
    let step_bar = create_step_bar("🔧 Montando áudio final...");
    let temp_dubbed_wav = temp_path.join("dubbed_audio.wav");

    let synced_refs: Vec<(usize, &std::path::Path)> = synced_paths
        .iter()
        .map(|(i, p)| (*i, p.as_path()))
        .collect();

    assemble::assemble_segments(
        &synced_refs,
        &merged_segments,
        total_duration,
        &temp_dubbed_wav,
        temp_path,
    )?;

    step_bar.finish_with_message("🔧 Montagem concluída");

    // ═══════════════════════════════════════════
    // ESTÁGIO 7: Gerar saída final (MP4 ou WAV)
    // ═══════════════════════════════════════════
    let is_video_output = config.output_path.extension()
        .map(|ext| ext.eq_ignore_ascii_case("mp4"))
        .unwrap_or(false);

    if is_video_output {
        let step_bar = create_step_bar("🎬 Incluindo áudio dublado no vídeo...");

        extract::merge_audio_into_video(
            &config.input_path,
            &temp_dubbed_wav,
            &config.output_path,
        )?;

        step_bar.finish_with_message("🎬 Vídeo dublado gerado com sucesso!");
    } else {
        // Copiar WAV para o destino final
        std::fs::copy(&temp_dubbed_wav, &config.output_path)?;
    }

    // ═══════════════════════════════════════════
    // Limpeza: remover caches de transcrição e tradução
    // ═══════════════════════════════════════════
    let cache_transcription = cache_dir.join(CACHE_TRANSCRIPTION_FILE);
    let cache_translation = cache_dir.join(CACHE_TRANSLATION_FILE);
    let mut cleaned = Vec::new();

    if cache_transcription.exists() {
        std::fs::remove_file(&cache_transcription).ok();
        cleaned.push(CACHE_TRANSCRIPTION_FILE);
    }
    if cache_translation.exists() {
        std::fs::remove_file(&cache_translation).ok();
        cleaned.push(CACHE_TRANSLATION_FILE);
    }
    if !cleaned.is_empty() {
        println!("🧹 Cache removido: {}", cleaned.join(", "));
    }

    Ok(())
}

/// Agrupa segmentos curtos (< MIN_SEGMENT_DURATION_MS) com o segmento seguinte.
/// Isso evita que o modelo TTS tenha problemas com textos muito curtos como "Certo?" ou "Sim."
fn merge_short_segments(segments: &[Segment]) -> Vec<Segment> {
    if segments.is_empty() {
        return Vec::new();
    }

    let mut result: Vec<Segment> = Vec::new();
    let mut accumulator: Option<Segment> = None;

    for seg in segments {
        match accumulator.take() {
            Some(mut acc) => {
                // O acumulado era curto — usar tolerância maior para segmentos muito curtos (<1s)
                let gap_ms = seg.start_ms.saturating_sub(acc.end_ms);
                let max_gap = if acc.duration_ms() < 1000 { 2000 } else { 500 };

                if gap_ms <= max_gap {
                    // Mesclar: expandir o acumulado para cobrir este segmento
                    acc.end_ms = seg.end_ms;
                    acc.text = format!("{} {}", acc.text, seg.text);
                    acc.translated_text =
                        format!("{} {}", acc.translated_text, seg.translated_text);

                    // Se o resultado mesclado ainda é curto, continuar acumulando
                    if acc.duration_ms() < MIN_SEGMENT_DURATION_MS {
                        accumulator = Some(acc);
                    } else {
                        result.push(acc);
                    }
                } else {
                    // Gap grande demais, emitir o acumulado sozinho e começar de novo
                    result.push(acc);
                    if seg.duration_ms() < MIN_SEGMENT_DURATION_MS {
                        accumulator = Some(seg.clone());
                    } else {
                        result.push(seg.clone());
                    }
                }
            }
            None => {
                if seg.duration_ms() < MIN_SEGMENT_DURATION_MS {
                    accumulator = Some(seg.clone());
                } else {
                    result.push(seg.clone());
                }
            }
        }
    }

    // Flush do acumulador restante
    if let Some(acc) = accumulator {
        // Tentar mesclar com o último segmento do resultado
        if let Some(last) = result.last_mut() {
            let gap_ms = acc.start_ms.saturating_sub(last.end_ms);
            if gap_ms <= 500 {
                last.end_ms = acc.end_ms;
                last.text = format!("{} {}", last.text, acc.text);
                last.translated_text =
                    format!("{} {}", last.translated_text, acc.translated_text);
            } else {
                result.push(acc);
            }
        } else {
            result.push(acc);
        }
    }

    result
}

/// Cria uma progress bar com spinner
fn create_step_bar(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

/// Cria uma progress bar com contagem
fn create_progress_bar(total: u64, prefix: &str) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{prefix} [{bar:30.cyan/dim}] {pos}/{len} ({eta} restante)",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    pb.set_prefix(prefix.to_string());
    pb
}

/// Remove segmentos sobrepostos mantendo apenas os não-sobrepostos.
/// Assume que os segmentos já estão ordenados por start_ms.
fn deduplicate_segments(segments: Vec<Segment>) -> Vec<Segment> {
    if segments.is_empty() {
        return segments;
    }

    let mut result: Vec<Segment> = Vec::new();
    result.push(segments[0].clone());

    for seg in segments.iter().skip(1) {
        let last = result.last().unwrap();
        // Se este segmento começa depois do último terminar, adicionar
        if seg.start_ms >= last.end_ms {
            result.push(seg.clone());
        }
        // Se sobrepõe significativamente, descartar (é duplicata de chunk boundary)
    }

    result
}
