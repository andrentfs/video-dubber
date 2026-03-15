# Guia de Uso

## Setup Inicial

### 1. Instalar pré-requisitos

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# ffmpeg
brew install ffmpeg
```

### 2. Compilar o projeto

```bash
cd video-dubber
cargo build --release
```

### 3. Configurar API Key

Opção A — variável de ambiente:
```bash
export OPENROUTER_API_KEY="sk-or-v1-..."
```

Opção B — arquivo `.env`:
```bash
echo "OPENROUTER_API_KEY=sk-or-v1-..." > .env
```

Opção C — argumento CLI:
```bash
cargo run --release -- --api-key "sk-or-v1-..." --input video.mp4
```

---

## Cenários de Uso

### Vídeo curto (< 5 min)

```bash
cargo run --release -- -i apresentacao.mp4 -o apresentacao_ptbr.wav
```

- Sem chunking (um request de transcrição)
- ~30 segundos de processamento

### Vídeo longo (15 min+)

```bash
cargo run --release -- \
  -i aula_completa.mp4 \
  -o aula_ptbr.wav \
  --chunk-duration 180 \
  --max-concurrent 8
```

- Chunks de 3 min (menos payload por request)
- 8 TTS requests simultâneos (mais rápido)
- ~3-5 minutos de processamento

### Escolher voz específica

```bash
# Voz masculina grave
cargo run --release -- -i video.mp4 -o video_ptbr.wav --voice onyx

# Voz feminina suave
cargo run --release -- -i video.mp4 -o video_ptbr.wav --voice shimmer
```

---

## Cache e Retomada

O pipeline salva resultados intermediários automaticamente:

| Arquivo | Conteúdo |
|---------|----------|
| `cache_transcription.json` | Transcrição com timestamps |
| `cache_translation.json` | Tradução em PT-BR |

### Se o TTS falhar no meio

Basta rodar novamente — o programa detecta os caches e pula direto para a etapa de TTS:

```bash
# Primeira execução: falhou no segmento 87 de 150
cargo run --release -- -i video.mp4 -o output.wav

# Segunda execução: reutiliza transcrição e tradução do cache
cargo run --release -- -i video.mp4 -o output.wav
```

### Forçar re-processamento completo

Delete os caches antes de rodar:
```bash
rm -f cache_transcription.json cache_translation.json
cargo run --release -- -i video.mp4 -o output.wav
```

---

## Troubleshooting

### "ffmpeg not found"

```bash
brew install ffmpeg
# ou no Linux: sudo apt install ffmpeg
```

### "OpenRouter API error (401)"

API key inválida. Verifique:
```bash
echo $OPENROUTER_API_KEY
```

### "OpenRouter API error (429)"

Rate limiting. Reduza o paralelismo:
```bash
cargo run --release -- -i video.mp4 -o output.wav --max-concurrent 2
```

### TTS retorna áudio vazio

O segmento pode ser muito curto ou ter caracteres especiais. O programa vai falhar com mensagem indicando qual texto causou o problema.

### Áudio dessincronizado

Se a velocidade parece estranha:
- Verifique `cache_transcription.json` — os timestamps estão corretos?
- Tente com `--chunk-duration 120` para chunks menores (transcrição mais precisa)

---

## Output

O programa gera um arquivo **WAV** com o áudio dublado. Para converter para outros formatos:

```bash
# WAV → MP3
ffmpeg -i output.wav -codec:libmp3lame -qscale:a 2 output.mp3

# Substituir áudio no vídeo original
ffmpeg -i video_original.mp4 -i output.wav \
  -c:v copy -map 0:v:0 -map 1:a:0 \
  video_dubbed.mp4
```
