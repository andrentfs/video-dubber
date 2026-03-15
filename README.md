# Video Dubber CLI

> Ferramenta de linha de comando em Rust para dublagem automática de vídeos usando IA.

Extrai o áudio de um vídeo MP4, transcreve, traduz para português brasileiro e gera um novo áudio dublado sincronizado com o timing original da fala — tudo via [OpenRouter API](https://openrouter.ai).

---

## ✨ Features

- 🎙️ **Transcrição com timestamps** via Gemini 2.5 Flash
- 🌐 **Tradução contextual** via GPT-4.1-mini (lotes com contexto completo)
- 🔊 **TTS natural** via GPT-4o-mini-tts (6 vozes disponíveis)
- ⏱️ **Sincronização de timing** — cada segmento dublado respeita a duração original
- ✂️ **Chunking automático** — vídeos longos são divididos em partes de 5 min
- ⚡ **TTS paralelo** — até 5 requests simultâneos (configurável)
- 💾 **Cache intermediário** — transcrição e tradução salvas em JSON para retomar em caso de falha

## Pré-requisitos

| Ferramenta | Instalação |
|-----------|-----------|
| Rust (1.75+) | [rustup.rs](https://rustup.rs) |
| ffmpeg + ffprobe | `brew install ffmpeg` |
| Conta OpenRouter | [openrouter.ai](https://openrouter.ai) |

## Instalação

```bash
git clone <repo-url> video-dubber
cd video-dubber
cargo build --release
```

## Uso Rápido

```bash
# Configurar API key
export OPENROUTER_API_KEY="sua_chave_aqui"

# Dublar um vídeo
cargo run --release -- --input video.mp4 --output dubbed.wav

# Com opções
cargo run --release -- \
  --input video.mp4 \
  --output dubbed.wav \
  --voice nova \
  --max-concurrent 10 \
  --chunk-duration 180
```

## Opções da CLI

| Argumento | Descrição | Default |
|-----------|-----------|---------|
| `--input`, `-i` | Caminho do vídeo MP4 (obrigatório) | — |
| `--output`, `-o` | Caminho do áudio de saída | `output_dubbed.wav` |
| `--voice`, `-v` | Voz do TTS | `nova` |
| `--api-key` | Chave API OpenRouter | env `OPENROUTER_API_KEY` |
| `--max-concurrent` | Requests TTS simultâneos | `5` |
| `--chunk-duration` | Duração máxima por chunk (segundos) | `300` |

### Vozes disponíveis

| Voz | Descrição |
|-----|-----------|
| `alloy` | Neutra, versátil |
| `echo` | Masculina, grave |
| `fable` | Narrativa, expressiva |
| `onyx` | Masculina, profunda |
| `nova` | Feminina, natural |
| `shimmer` | Feminina, suave |

## Documentação

- [Arquitetura](./docs/architecture.md) — Pipeline, fluxo de dados e decisões técnicas
- [API Reference](./docs/api-reference.md) — Endpoints OpenRouter utilizados
- [Guia de Uso](./docs/usage-guide.md) — Cenários práticos e troubleshooting

## Licença

MIT
