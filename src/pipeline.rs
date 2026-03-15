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
    // ESTÁGIO 2: Dividir em chunks (com overlap)
    // ═══════════════════════════════════════════
    let chunks: Vec<(PathBuf, u64)> = if total_duration > config.chunk_duration_secs as f64 {
        let step_bar = create_step_bar("✂️  Dividindo áudio em chunks...");
        let chunk_data = extract::split_audio_into_chunks(
            &full_wav,
            config.chunk_duration_secs,
            config.chunk_overlap_secs,
            temp_path,
        )?;
        step_bar.finish_with_message(format!(
            "✂️  Dividido em {} chunks ({}s com {}s overlap)",
            chunk_data.len(),
            config.chunk_duration_secs,
            config.chunk_overlap_secs
        ));
        chunk_data
    } else {
        vec![(full_wav.clone(), 0)]
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
            for (i, (chunk_path, offset_ms)) in chunks.iter().enumerate() {
                // Obter duração real do chunk para dar ao Gemini uma âncora temporal
                let chunk_duration_hint = extract::get_audio_duration_secs(chunk_path).ok();

                // Retry na transcrição: até 2 tentativas adicionais
                let mut chunk_segments = None;
                let mut last_err = String::new();
                for attempt in 0..3 {
                    match transcribe::transcribe_audio_chunk(&client, chunk_path, *offset_ms, chunk_duration_hint).await {
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

            // Remover segmentos sobrepostos (merge inteligente com overlap de chunks)
            all_segments = deduplicate_segments(all_segments);

            pb.finish_with_message(format!(
                "🎙️  Transcritos {} segmentos",
                all_segments.len()
            ));

            // Validar e corrigir timestamps
            all_segments = validate_segment_timestamps(all_segments, total_duration);

            // Salvar cache
            let cache_json = serde_json::to_string_pretty(&all_segments)?;
            std::fs::write(&cache_path, cache_json).ok();

            all_segments
        }
    };

    if segments.is_empty() {
        anyhow::bail!("No speech segments were detected in the audio.");
    }

    // Diagnóstico de cobertura pós-transcrição
    {
        let first_start = segments.first().map(|s| s.start_ms).unwrap_or(0);
        let last_end = segments.last().map(|s| s.end_ms).unwrap_or(0);
        let total_covered: u64 = segments.iter().map(|s| s.duration_ms()).sum();
        let mut gaps_over_1s = 0;
        for w in segments.windows(2) {
            let gap = w[1].start_ms.saturating_sub(w[0].end_ms);
            if gap > 1000 {
                gaps_over_1s += 1;
            }
        }
        println!(
            "   → {} segmentos detectados (cobertura: {:.1}s-{:.1}s, fala: {:.1}s, gaps>1s: {})",
            segments.len(),
            first_start as f64 / 1000.0,
            last_end as f64 / 1000.0,
            total_covered as f64 / 1000.0,
            gaps_over_1s,
        );
    }

    if config.debug_segments {
        let debug_path = cache_dir.join("debug_01_transcription.json");
        let debug_json = serde_json::to_string_pretty(&segments)?;
        std::fs::write(&debug_path, debug_json).ok();
        println!("   🐛 Debug: transcrição salva em {:?}", debug_path);
    }

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

    if config.debug_segments {
        let debug_path = cache_dir.join("debug_02_translation.json");
        let debug_json = serde_json::to_string_pretty(&translated_segments)?;
        std::fs::write(&debug_path, debug_json).ok();
        println!("   🐛 Debug: tradução salva em {:?}", debug_path);
    }

    // ═══════════════════════════════════════════
    // ESTÁGIO 4.5: Agrupar segmentos curtos
    // ═══════════════════════════════════════════
    let mut merged_segments = merge_short_segments(&translated_segments);
    if merged_segments.len() < translated_segments.len() {
        println!(
            "   → {} segmentos agrupados (de {} originais) para estabilidade do TTS",
            merged_segments.len(),
            translated_segments.len()
        );
    }

    // ═══════════════════════════════════════════
    // ESTÁGIO 4.7: Validar e corrigir traduções longas
    // ═══════════════════════════════════════════
    let retranslated = translate::validate_and_fix_translations(&client, &mut merged_segments).await?;
    if retranslated > 0 {
        println!("   🔄 {} segmento(s) re-traduzido(s) para caber no tempo", retranslated);
    }

    if config.debug_segments {
        let debug_path = cache_dir.join("debug_03_merged.json");
        let debug_json = serde_json::to_string_pretty(&merged_segments)?;
        std::fs::write(&debug_path, debug_json).ok();
        println!("   🐛 Debug: segmentos mesclados salvos em {:?}", debug_path);
    }

    // ═══════════════════════════════════════════
    // ESTÁGIO 5: TTS com paralelismo (GPT-4o-mini-tts)
    // ═══════════════════════════════════════════
    let pb = create_progress_bar(merged_segments.len() as u64, "🔊 Gerando áudio");
    let semaphore = Arc::new(Semaphore::new(config.max_concurrent_tts));

    // Pré-calcular gap após cada segmento para overflow de silêncio
    let segment_gaps: Vec<f64> = merged_segments
        .iter()
        .enumerate()
        .map(|(i, seg)| {
            if i + 1 < merged_segments.len() {
                merged_segments[i + 1].start_ms.saturating_sub(seg.end_ms) as f64 / 1000.0
            } else {
                0.0
            }
        })
        .collect();

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
            let gap_after_secs = segment_gaps[i];
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
                            match extract::adjust_audio_speed_with_overflow(
                                &raw_path, target_duration, gap_after_secs, &synced_path
                            ) {
                                Ok(effective_duration) => {
                                    pb.inc(1);
                                    return Ok::<(usize, Option<PathBuf>, Option<f64>), anyhow::Error>((i, Some(synced_path), Some(effective_duration)));
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
                Ok::<(usize, Option<PathBuf>, Option<f64>), anyhow::Error>((i, None, None))
            })
        })
        .collect();

    // Aguardar todas as tasks de TTS
    let mut synced_paths: Vec<(usize, PathBuf)> = Vec::new();
    let mut failed_count: usize = 0;

    for task in tts_tasks {
        match task.await.context("TTS task panicked")? {
            Ok((idx, Some(path), effective_dur)) => {
                // Salvar duração efetiva no segmento para uso na montagem
                if let Some(dur) = effective_dur {
                    merged_segments[idx].effective_duration_secs = Some(dur);
                }
                synced_paths.push((idx, path));
            }
            Ok((_idx, None, _)) => failed_count += 1,
            Err(e) => {
                eprintln!("❌ Erro crítico em task TTS: {}", e);
                failed_count += 1;
            }
        }
    }

    // Verificar colisões de overflow: se um segmento com duração efetiva invade o próximo
    for i in 0..merged_segments.len().saturating_sub(1) {
        if let Some(eff_dur) = merged_segments[i].effective_duration_secs {
            let eff_end_ms = merged_segments[i].start_ms + (eff_dur * 1000.0) as u64;
            let next_start_ms = merged_segments[i + 1].start_ms;
            if eff_end_ms > next_start_ms {
                let collision_ms = eff_end_ms - next_start_ms;
                eprintln!(
                    "⚠️  Segmento {} overflow colide com seg {} por {}ms — será cortado na montagem",
                    i, i + 1, collision_ms
                );
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
        let mut drift_stats: Vec<(usize, f64, u64)> = Vec::new(); // (idx, drift_ms, position_ms)
        for &(idx, ref path) in &synced_paths {
            if let Ok(actual) = extract::get_audio_duration_secs(path) {
                let target = merged_segments[idx].effective_duration_secs
                    .unwrap_or_else(|| merged_segments[idx].duration_secs());
                let drift_ms = (actual - target).abs() * 1000.0;
                drift_stats.push((idx, drift_ms, merged_segments[idx].start_ms));
            }
        }
        if !drift_stats.is_empty() {
            let avg_drift = drift_stats.iter().map(|(_, d, _)| d).sum::<f64>() / drift_stats.len() as f64;
            let max_drift = drift_stats.iter().map(|(_, d, _)| *d).fold(0.0_f64, f64::max);
            let over_50ms = drift_stats.iter().filter(|(_, d, _)| *d > 50.0).count();
            println!(
                "   📊 Drift stats: avg={:.1}ms, max={:.1}ms, {} com >50ms drift ({} segmentos)",
                avg_drift, max_drift, over_50ms, drift_stats.len()
            );
            // Logar segmentos com drift > 100ms
            for (idx, drift_ms, pos_ms) in &drift_stats {
                if *drift_ms > 100.0 {
                    eprintln!(
                        "   ⚠️  Drift >100ms: seg {} em {:.1}s → {:.0}ms drift",
                        idx, *pos_ms as f64 / 1000.0, drift_ms
                    );
                }
            }
        }
    }

    if config.debug_segments {
        let debug_path = cache_dir.join("debug_04_post_tts.json");
        let debug_json = serde_json::to_string_pretty(&merged_segments)?;
        std::fs::write(&debug_path, debug_json).ok();
        println!("   🐛 Debug: segmentos pós-TTS salvos em {:?}", debug_path);
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
                    // Verificar se merge criaria segmento com taxa de fala inviável
                    let merged_len = acc.translated_text.len() + 1 + seg.translated_text.len();
                    let merged_duration = (seg.end_ms - acc.start_ms) as f64 / 1000.0;
                    let would_be_too_fast = merged_duration > 0.0
                        && (merged_len as f64 / merged_duration) > 14.0;

                    if would_be_too_fast {
                        // Merge criaria segmento rápido demais — emitir acumulado separado
                        result.push(acc);
                        if seg.duration_ms() < MIN_SEGMENT_DURATION_MS {
                            accumulator = Some(seg.clone());
                        } else {
                            result.push(seg.clone());
                        }
                    } else {
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

/// Remove segmentos sobrepostos com merge inteligente.
/// Quando dois segmentos se sobrepõem:
/// - Se texto similar (>50% words em comum): manter o que está mais centralizado no chunk
/// - Se texto diferente: dividir no ponto médio da sobreposição
/// Assume que os segmentos já estão ordenados por start_ms.
fn deduplicate_segments(segments: Vec<Segment>) -> Vec<Segment> {
    if segments.is_empty() {
        return segments;
    }

    let mut result: Vec<Segment> = Vec::new();
    result.push(segments[0].clone());

    for seg in segments.iter().skip(1) {
        let last = result.last_mut().unwrap();

        // Se não há sobreposição, adicionar diretamente
        if seg.start_ms >= last.end_ms {
            result.push(seg.clone());
            continue;
        }

        // Há sobreposição — decidir como resolver
        let overlap_ms = last.end_ms.saturating_sub(seg.start_ms);

        // Se sobreposição mínima (< 100ms), apenas ajustar boundary
        if overlap_ms < 100 {
            let mut new_seg = seg.clone();
            new_seg.start_ms = last.end_ms;
            if new_seg.end_ms > new_seg.start_ms {
                result.push(new_seg);
            }
            continue;
        }

        // Verificar similaridade de texto
        let sim = text_similarity(&last.text, &seg.text);

        if sim > 0.5 {
            // Texto similar — duplicata de chunk boundary
            // Manter o segmento mais longo (geralmente mais preciso)
            if seg.duration_ms() > last.duration_ms() {
                // Substituir o último pelo novo, mas sem voltar antes do start do último
                let mut new_seg = seg.clone();
                new_seg.start_ms = new_seg.start_ms.max(last.start_ms);
                *last = new_seg;
            }
            // Caso contrário, manter o último (já no result) e descartar seg
        } else {
            // Texto diferente — dividir no ponto médio da sobreposição
            let midpoint = (last.end_ms + seg.start_ms) / 2;
            last.end_ms = midpoint;
            let mut new_seg = seg.clone();
            new_seg.start_ms = midpoint;
            if new_seg.end_ms > new_seg.start_ms && last.end_ms > last.start_ms {
                result.push(new_seg);
            }
        }
    }

    result
}

/// Similaridade simples entre dois textos baseada em palavras em comum (Jaccard)
fn text_similarity(a: &str, b: &str) -> f64 {
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        return 0.0;
    }

    intersection as f64 / union as f64
}

/// Valida e corrige timestamps dos segmentos pós-transcrição.
/// - Garante monotonia (seg[i].end_ms <= seg[i+1].start_ms)
/// - Divide segmentos > 15s
/// - Loga warnings para gaps > 5s e segmentos < 300ms
fn validate_segment_timestamps(segments: Vec<Segment>, total_duration_secs: f64) -> Vec<Segment> {
    if segments.is_empty() {
        return segments;
    }

    let mut result: Vec<Segment> = Vec::new();

    // Primeiro: dividir segmentos muito longos (> 15s)
    for seg in &segments {
        if seg.duration_ms() > 15000 {
            let mid = (seg.start_ms + seg.end_ms) / 2;
            // Tentar dividir o texto ao meio (pela palavra mais próxima do meio)
            let words: Vec<&str> = seg.text.split_whitespace().collect();
            let (text1, text2) = if words.len() >= 2 {
                let mid_idx = words.len() / 2;
                (
                    words[..mid_idx].join(" "),
                    words[mid_idx..].join(" "),
                )
            } else {
                (seg.text.clone(), String::new())
            };

            result.push(Segment {
                start_ms: seg.start_ms,
                end_ms: mid,
                text: text1,
                translated_text: seg.translated_text.clone(),
                effective_duration_secs: None,
            });
            if !text2.is_empty() {
                result.push(Segment {
                    start_ms: mid,
                    end_ms: seg.end_ms,
                    text: text2,
                    translated_text: String::new(),
                    effective_duration_secs: None,
                });
            }
            eprintln!(
                "⚠️  Segmento de {:.1}s dividido ao meio em {:.1}s",
                seg.duration_secs(),
                seg.duration_secs() / 2.0
            );
        } else {
            result.push(seg.clone());
        }
    }

    // Segundo: garantir monotonia
    for i in 1..result.len() {
        if result[i].start_ms < result[i - 1].end_ms {
            let midpoint = (result[i - 1].end_ms + result[i].start_ms) / 2;
            result[i - 1].end_ms = midpoint;
            result[i].start_ms = midpoint;
        }
    }

    // Terceiro: logar warnings
    for seg in &result {
        if seg.duration_ms() < 300 {
            eprintln!(
                "⚠️  Segmento muito curto: {}ms em {:.1}s: \"{}\"",
                seg.duration_ms(),
                seg.start_ms as f64 / 1000.0,
                &seg.text[..seg.text.len().min(40)]
            );
        }
    }

    for w in result.windows(2) {
        let gap_ms = w[1].start_ms.saturating_sub(w[0].end_ms);
        if gap_ms > 5000 {
            eprintln!(
                "⚠️  Gap de {:.1}s entre segmentos em {:.1}s-{:.1}s (possível fala perdida)",
                gap_ms as f64 / 1000.0,
                w[0].end_ms as f64 / 1000.0,
                w[1].start_ms as f64 / 1000.0,
            );
        }
    }

    // Verificar cobertura
    let total_covered: u64 = result.iter().map(|s| s.duration_ms()).sum();
    let total_ms = (total_duration_secs * 1000.0) as u64;
    if total_ms > 0 {
        let coverage = total_covered as f64 / total_ms as f64;
        if coverage < 0.6 {
            eprintln!(
                "⚠️  Cobertura baixa: {:.0}% — possível retranscrição necessária",
                coverage * 100.0
            );
        }
    }

    result
}
