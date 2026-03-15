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

There are no tests in this project currently.

## Prerequisites

- **ffmpeg + ffprobe** must be installed (`brew install ffmpeg`)
- **OPENROUTER_API_KEY** set in `.env` or passed via `--api-key`

## Architecture

The codebase follows a 7-stage sequential pipeline (`src/pipeline.rs`):

1. **Extract audio** — ffmpeg extracts WAV (16kHz mono) from input MP4
2. **Chunk splitting** — Long audio (>5min default) split into chunks for API limits
3. **Transcription** — Each chunk sent as base64 WAV to `google/gemini-2.5-flash` via OpenRouter; returns segments with `start_ms`/`end_ms` timestamps
4. **Translation** — All segments sent in batches of 30 to `openai/gpt-4.1-mini`; translates to PT-BR preserving timestamps
5. **Short segment merging** — Segments <2s are merged with adjacent ones (gap ≤500ms) to avoid TTS issues with very short text
6. **TTS generation** — Parallel requests (semaphore-controlled, default 5) to `openai/gpt-audio-mini` with SSE streaming; PCM16→WAV conversion; speed-adjusted via ffmpeg `atempo` to match original segment duration
7. **Assembly** — ffmpeg concat demuxer stitches segments with silence gaps; if output is `.mp4`, dubbed audio is muxed back into the original video (video stream copied, no re-encoding)

### Key modules

| Module | Role |
|--------|------|
| `src/main.rs` | CLI (clap) argument parsing, config setup |
| `src/models.rs` | `Segment` (timestamp + text + translation) and `Config` structs |
| `src/pipeline.rs` | Orchestrates all stages, caching, progress bars, segment merging |
| `src/openrouter/client.rs` | Shared HTTP client with auth headers, `chat_completion` method |
| `src/openrouter/transcribe.rs` | Audio→base64→Gemini multimodal transcription |
| `src/openrouter/translate.rs` | Batch translation via GPT-4.1-mini with JSON response format |
| `src/openrouter/tts.rs` | SSE streaming TTS, PCM16 base64 decoding, truncation detection |
| `src/audio/extract.rs` | All ffmpeg operations: extract, split, speed adjust, merge audio+video |
| `src/audio/assemble.rs` | Builds concat list with silence gaps, final ffmpeg concat |

### Caching

Transcription and translation results are cached as JSON files (`cache_transcription.json`, `cache_translation.json`) in the output directory. Delete these to force re-processing.

### Error handling

TTS uses retry with exponential backoff (3 attempts). Pipeline aborts if >10% of TTS segments fail. All ffmpeg calls use `anyhow::Context` for error propagation.

### All API calls go through OpenRouter

The project does not call OpenAI/Google directly. Every model (Gemini, GPT-4.1-mini, GPT-audio-mini) is accessed through `https://openrouter.ai/api/v1/chat/completions`.
