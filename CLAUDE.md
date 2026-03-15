# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Rust CLI tool that automatically dubs videos by extracting audio, transcribing, translating to Brazilian Portuguese, and generating dubbed audio — all via OpenRouter API. The project is in Portuguese (PT-BR) context: comments, UI messages, and the target translation language are PT-BR.

## Build & Run Commands

```bash
cargo build --release
cargo run --release -- --input video.mp4 --output dubbed.mp4
cargo run --release -- --input video.mp4 --output dubbed.wav --voice nova --max-concurrent 10
```

### CLI Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `--input, -i` | (required) | Input MP4 file path |
| `--output, -o` | `output_dubbed.mp4` | Output file (.mp4 or .wav) |
| `--voice, -v` | `onyx` | TTS voice: alloy, echo, fable, onyx, nova, shimmer |
| `--api-key` | env `OPENROUTER_API_KEY` | OpenRouter API key |
| `--max-concurrent` | `5` | Max parallel TTS requests |
| `--chunk-duration` | `60` | Max audio chunk duration in seconds |
| `--chunk-overlap` | `10` | Overlap in seconds between adjacent chunks |
| `--debug-segments` | `false` | Dump detailed segment JSON at each pipeline stage |

There are no tests in this project currently.

## Prerequisites

- **ffmpeg + ffprobe** must be installed (`brew install ffmpeg`)
- **OPENROUTER_API_KEY** set in `.env` or passed via `--api-key`

## Architecture

The codebase follows a 7-stage sequential pipeline (`src/pipeline.rs`, ~520 lines):

1. **Extract audio** — ffmpeg extracts WAV (16kHz mono) from input MP4
2. **Chunk splitting** — Long audio (>1min default) split into overlapping chunks (10s overlap) for better timestamp precision; stride = chunk_duration - overlap
3. **Transcription** — Each chunk sent as base64 WAV to `google/gemini-2.5-flash` via OpenRouter with duration hint anchor; returns segments with `start_ms`/`end_ms` timestamps. Retries up to 3x with exponential backoff. Smart deduplication: similar overlapping segments merged by keeping longest, different text split at midpoint. Post-transcription timestamp validation ensures monotonicity and splits segments >15s
4. **Translation** — Segments sent in batches of 30 to `openai/gpt-4.1` via OpenRouter; translates to PT-BR colloquial speech preserving timestamps. Temperature: 0.4
5. **Short segment merging** — Segments <2s merged with adjacent ones (gap ≤500ms, relaxed to 2000ms for <1s segments). Warns when translated text exceeds ~14 chars/sec for the segment duration
6. **TTS generation** — Parallel requests (semaphore-controlled, default 5) to `openai/gpt-audio-mini` via OpenRouter with SSE streaming; incremental PCM16 base64 decoding (24kHz mono); speed-adjusted via ffmpeg `atempo` chain to match original segment duration (clamped 0.5x–1.8x); `apad+atrim` forces exact target duration. Captures effective duration from overflow borrowing. Retries up to 3x with jitter. Reports drift stats with per-segment detail for drifts >100ms. Detects overflow collisions between adjacent segments
7. **Assembly** — `adelay+amix` approach: generates silence baseline matching total duration, then positions each segment at its absolute `start_ms` via `adelay` filter with `atrim` to prevent overflow collisions, fade-in (10ms) and fade-out (15ms). Uses effective_duration_secs when available. Batches of 40 inputs per ffmpeg invocation to avoid filter limits. Uses `dynaudnorm` for smooth leveling. If output is `.mp4`, dubbed audio is muxed back into the original video (`-c:v copy`, no re-encoding)

### Key modules

| Module | Role |
|--------|------|
| `src/main.rs` | CLI (clap) argument parsing, config setup |
| `src/lib.rs` | Library exports (pub mod declarations) |
| `src/models.rs` | `Segment` (timestamp + text + translation) and `Config` structs |
| `src/pipeline.rs` | Orchestrates all 7 stages, caching, progress bars, segment merging, drift diagnostics |
| `src/openrouter/client.rs` | Shared HTTP client with auth headers, 5-min timeout, `chat_completion` method |
| `src/openrouter/transcribe.rs` | Audio→base64→Gemini multimodal transcription with retry |
| `src/openrouter/translate.rs` | Batch translation (30 segments) via GPT-4.1 with JSON response format |
| `src/openrouter/tts.rs` | SSE streaming TTS, incremental PCM16 base64 decoding, truncation detection, PCM→WAV conversion |
| `src/audio/extract.rs` | ffmpeg operations: extract, split, speed adjust (`atempo` chain + pitch compensation), merge audio+video |
| `src/audio/assemble.rs` | Builds silence baseline, positions segments via `adelay+amix` in batches, `dynaudnorm` normalization |

### Caching

Transcription and translation results are cached as JSON files (`cache_transcription.json`, `cache_translation.json`) in the output directory. Caches are automatically deleted after successful pipeline completion. Delete manually to force re-processing during development.

### Error handling

- **Transcription**: Up to 3 retries with exponential backoff (2s × 2^attempt)
- **TTS**: Up to 3 retries per segment with exponential backoff + jitter (±30%). Pipeline aborts if >10% of TTS segments fail
- **Speed adjustment**: Clamped to [0.5x, 2.5x] with warnings when limits hit
- All ffmpeg calls use `anyhow::Context` for error propagation

### All API calls go through OpenRouter

The project does not call OpenAI/Google directly. Every model is accessed through `https://openrouter.ai/api/v1/chat/completions`:

| Model | OpenRouter ID | Purpose | Temperature |
|-------|---------------|---------|-------------|
| Gemini 2.5 Flash | `google/gemini-2.5-flash` | Transcription | 0.1 |
| GPT-4.1 | `openai/gpt-4.1` | Translation | 0.4 |
| GPT-4o-mini-tts | `openai/gpt-audio-mini` | Text-to-speech | 0.3 |
