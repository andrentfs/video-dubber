# API Reference — OpenRouter

Todas as chamadas passam pelo [OpenRouter](https://openrouter.ai) como gateway.

**Base URL**: `https://openrouter.ai/api/v1`

**Autenticação**: Header `Authorization: Bearer <OPENROUTER_API_KEY>`

---

## 1. Transcrição — Gemini 2.5 Flash

**Endpoint**: `POST /chat/completions`
**Modelo**: `google/gemini-2.5-flash`

### Request

```json
{
  "model": "google/gemini-2.5-flash",
  "messages": [
    {
      "role": "user",
      "content": [
        {
          "type": "text",
          "text": "Transcreva este áudio com timestamps em JSON..."
        },
        {
          "type": "input_audio",
          "input_audio": {
            "data": "<base64_encoded_wav>",
            "format": "wav"
          }
        }
      ]
    }
  ],
  "temperature": 0.1,
  "response_format": { "type": "json_object" }
}
```

### Response (conteúdo extraído de `choices[0].message.content`)

```json
{
  "segments": [
    {
      "start_ms": 0,
      "end_ms": 3200,
      "text": "Hello, how are you today?"
    },
    {
      "start_ms": 3500,
      "end_ms": 6100,
      "text": "I'm doing great, thanks for asking."
    }
  ]
}
```

### Limites

- Áudio máximo: ~8.4 horas por request
- Recomendado: chunks de 5 min para payloads < 10 MB

---

## 2. Tradução — GPT-4.1-mini

**Endpoint**: `POST /chat/completions`
**Modelo**: `openai/gpt-4.1-mini`

### Request

```json
{
  "model": "openai/gpt-4.1-mini",
  "messages": [
    {
      "role": "user",
      "content": "Translate the following segments to PT-BR...\n\n[{\"start_ms\": 0, \"end_ms\": 3200, \"text\": \"Hello\"}]\n\nReturn JSON: {\"segments\": [{\"start_ms\": 0, \"end_ms\": 3200, \"text\": \"Hello\", \"translated\": \"Olá\"}]}"
    }
  ],
  "temperature": 0.3,
  "response_format": { "type": "json_object" }
}
```

### Response

```json
{
  "segments": [
    {
      "start_ms": 0,
      "end_ms": 3200,
      "text": "Hello, how are you today?",
      "translated": "Olá, como você está hoje?"
    }
  ]
}
```

### Limites

- Batch recomendado: ~30 segmentos por request
- Custo: ~$0.40/1M input tokens

---

## 3. TTS — GPT-4o-mini-tts

**Endpoint**: `POST /audio/speech`
**Modelo**: `openai/gpt-4o-mini-tts`

### Request

```json
{
  "model": "openai/gpt-4o-mini-tts",
  "input": "Olá, como você está hoje?",
  "voice": "nova",
  "response_format": "wav",
  "instructions": "Speak naturally in Brazilian Portuguese..."
}
```

### Response

Binário WAV (bytes de áudio direto no body da resposta).

### Vozes disponíveis

| Voz | Perfil |
|-----|--------|
| `alloy` | Neutra, versátil |
| `echo` | Masculina, grave |
| `fable` | Narrativa, expressiva |
| `onyx` | Masculina, profunda |
| `nova` | Feminina, natural |
| `shimmer` | Feminina, suave |

### Limites

- Máximo ~2000 tokens de input por request
- 1 request por segmento de fala

---

## Estimativa de Custos

Para um vídeo de **15 minutos** (~150 segmentos):

| Modelo | Estimativa |
|--------|-----------|
| Gemini 2.5 Flash (transcrição) | ~$0.01–0.05 |
| GPT-4.1-mini (tradução) | ~$0.01 |
| GPT-4o-mini-tts (TTS × 150) | ~$0.10–0.30 |
| **Total** | **~$0.15–0.40** |
