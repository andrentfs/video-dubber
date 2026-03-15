# Arquitetura

## Pipeline de 6 Estágios

O Video Dubber segue um pipeline linear de processamento:

```
┌─────────┐    ┌─────────┐    ┌────────────┐    ┌───────────┐    ┌──────────┐    ┌──────────┐
│ Extract │───▶│  Chunk  │───▶│ Transcribe │───▶│ Translate │───▶│   TTS    │───▶│ Assemble │
│ (ffmpeg)│    │(ffmpeg) │    │(Gemini 2.5)│    │(GPT-4.1m) │    │(GPT-4o-m)│    │ (ffmpeg) │
└─────────┘    └─────────┘    └────────────┘    └───────────┘    └──────────┘    └──────────┘
  MP4→WAV       WAV→chunks     audio→JSON        text→PT-BR     PT-BR→WAV      segments→WAV
```

## Estágios Detalhados

### 1. Extract (audio/extract.rs)

Converte o vídeo MP4 em áudio WAV otimizado para transcrição.

- **Formato**: WAV, 16kHz, mono
- **Motivo do 16kHz**: É o sample rate padrão dos modelos de speech-to-text
- **Validação**: Verifica se `ffmpeg` está instalado antes de iniciar

### 2. Chunk (audio/extract.rs)

Divide o áudio em partes menores se a duração exceder o limite configurado.

- **Default**: 5 minutos (300s) por chunk
- **Motivo**: Evita payloads HTTP muito grandes (WAV 16kHz mono ≈ 1.9 MB/min)
- **Offset tracking**: Cada chunk recebe um offset de timestamp para manter a sincronização global

### 3. Transcribe (openrouter/transcribe.rs)

Envia cada chunk de áudio para o Gemini 2.5 Flash via OpenRouter.

- **Modelo**: `google/gemini-2.5-flash`
- **Input**: Áudio WAV codificado em base64
- **Output**: JSON com segmentos `{start_ms, end_ms, text}`
- **Prompt**: Instrui o modelo a retornar JSON estruturado com timestamps em milissegundos
- **Cache**: Resultado salvo em `cache_transcription.json`

### 4. Translate (openrouter/translate.rs)

Traduz todos os segmentos para português brasileiro.

- **Modelo**: `openai/gpt-4.1-mini`
- **Batch size**: 30 segmentos por request (balanceia contexto vs. custo)
- **Contexto**: Enviar em lotes garante tradução mais natural e consistente
- **Cache**: Resultado salvo em `cache_translation.json`

### 5. TTS — Text-to-Speech (openrouter/tts.rs)

Gera áudio para cada segmento traduzido.

- **Modelo**: `openai/gpt-4o-mini-tts`
- **Paralelismo**: Semáforo com N permits (default: 5 requests simultâneos)
- **Speed adjustment**: Após gerar, ajusta a velocidade com ffmpeg `atempo` para sincronizar com a duração original do segmento

#### Sincronização de Timing

Para cada segmento:
1. O TTS gera um áudio de duração `D_gerada`
2. A duração alvo é `D_original = end_ms - start_ms`
3. Fator de ajuste: `speed = D_gerada / D_original`
4. ffmpeg aplica `atempo=speed` (encadeia filtros para valores fora de [0.5, 2.0])

### 6. Assemble (audio/assemble.rs)

Concatena todos os segmentos sincronizados com gaps de silêncio.

- Usa ffmpeg concat demuxer
- Insere silêncio nos intervalos entre segmentos (gaps onde não há fala)
- Adiciona silêncio final se o último segmento termina antes do fim do vídeo

## Estrutura de Módulos

```
src/
├── main.rs              # CLI entry point (clap derive)
├── lib.rs               # Module re-exports
├── models.rs            # Segment, Config, response types
├── pipeline.rs          # Pipeline orchestrator (6 stages)
├── audio/
│   ├── mod.rs
│   ├── extract.rs       # ffmpeg: extract, chunk, speed adjust
│   └── assemble.rs      # ffmpeg: concat with silence gaps
└── openrouter/
    ├── mod.rs
    ├── client.rs         # Shared HTTP client (reqwest)
    ├── transcribe.rs     # Gemini 2.5 Flash
    ├── translate.rs      # GPT-4.1-mini
    └── tts.rs            # GPT-4o-mini-tts
```

## Fluxo de Dados

```
                    ┌──────────────────┐
                    │   Segment        │
                    │ ─────────────    │
                    │ start_ms: u64    │
                    │ end_ms: u64      │    Percorre todo o pipeline
                    │ text: String     │◄── como estrutura principal
                    │ translated: Str  │
                    └──────────────────┘

MP4 ──ffmpeg──▶ WAV ──base64──▶ Gemini ──JSON──▶ Vec<Segment>
                                                      │
                                                      ▼
                        WAV ◄──ffmpeg◄── GPT-4o-tts ◄── GPT-4.1-mini
                         │                                (tradução)
                         ▼
                    output.wav (final)
```

## Decisões Técnicas

| Decisão | Motivo |
|---------|--------|
| 3 modelos separados vs 1 | Cada modelo é otimizado para sua tarefa; tradução em lote dá mais contexto |
| Cache em JSON | Permite retomar o pipeline se o TTS falhar no meio (economia de custo) |
| Semáforo para TTS | Evita rate limiting da API; configurável pelo usuário |
| WAV 16kHz mono | Padrão de fato para modelos de speech-to-text |
| atempo encadeado | Filtro `atempo` do ffmpeg só aceita [0.5, 2.0], então encadeamos para fatores maiores |
| Tradução em lotes de 30 | Contexto suficiente para tradução natural, sem exceder limite de tokens |
