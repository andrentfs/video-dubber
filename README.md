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
| ffmpeg + ffprobe | macOS: `brew install ffmpeg` / Windows: ver seção abaixo |
| Conta OpenRouter | [openrouter.ai](https://openrouter.ai) |

## Instalação

```bash
git clone <repo-url> video-dubber
cd video-dubber
cargo build --release
```

## Uso Rápido (macOS / Linux)

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

---

## Uso no Windows

### 1. Instalar Rust

Baixe e execute o instalador oficial: [https://rustup.rs](https://rustup.rs)

Durante a instalação, se solicitado, instale também o **Visual Studio Build Tools** com o componente "Desktop development with C++".

Após instalar, abra um **novo terminal** (CMD ou PowerShell) e verifique:

```powershell
rustc --version
cargo --version
```

### 2. Instalar ffmpeg

**Opção A — winget (recomendado, Windows 10+):**

```powershell
winget install Gyan.FFmpeg
```

**Opção B — download manual:**

1. Acesse [https://www.gyan.dev/ffmpeg/builds/](https://www.gyan.dev/ffmpeg/builds/) e baixe o build **release full**
2. Extraia o ZIP em uma pasta, por exemplo `C:\ffmpeg`
3. Adicione `C:\ffmpeg\bin` ao **PATH** do sistema:
   - Abra o Menu Iniciar → pesquise **"Variáveis de Ambiente"**
   - Em **Variáveis do sistema**, selecione `Path` → **Editar** → **Novo**
   - Adicione `C:\ffmpeg\bin`
   - Clique **OK** em tudo e abra um **novo terminal**

Verifique a instalação:

```powershell
ffmpeg -version
ffprobe -version
```

### 3. Compilar o projeto

```powershell
git clone <repo-url> video-dubber
cd video-dubber
cargo build --release
```

O executável será gerado em: `target\release\video-dubber.exe`

### 4. Configurar a API Key (`.env`)

Crie um arquivo chamado `.env` **na mesma pasta de onde você vai executar o programa** (geralmente a raiz do projeto, ou a pasta onde está o `video-dubber.exe`).

O programa carrega o `.env` do **diretório de trabalho atual** (o diretório em que você está quando executa o comando).

```powershell
# Criar o arquivo .env na raiz do projeto
echo OPENROUTER_API_KEY=sk-or-v1-sua_chave_aqui > .env
```

O conteúdo do `.env` deve ser:

```
OPENROUTER_API_KEY=sk-or-v1-sua_chave_aqui
```

> **Nota:** Não use aspas ao redor do valor no `.env`. Não adicione espaços antes ou depois do `=`.

**Alternativas ao `.env`:**

```powershell
# Opção B — variável de ambiente (sessão atual do PowerShell)
$env:OPENROUTER_API_KEY = "sk-or-v1-sua_chave_aqui"

# Opção C — variável de ambiente permanente (requer novo terminal)
[System.Environment]::SetEnvironmentVariable("OPENROUTER_API_KEY", "sk-or-v1-sua_chave_aqui", "User")

# Opção D — passar direto na linha de comando
video-dubber.exe --api-key "sk-or-v1-sua_chave_aqui" --input video.mp4
```

### 5. Executar

**Via cargo (na raiz do projeto):**

```powershell
cargo run --release -- --input video.mp4 --output dubbed.mp4
```

**Via executável direto:**

```powershell
# Copie o .exe para a pasta desejada junto com o .env
copy target\release\video-dubber.exe C:\Users\SeuUsuario\Videos\

# Navegue até a pasta e execute
cd C:\Users\SeuUsuario\Videos\
video-dubber.exe --input video.mp4 --output dubbed.mp4 --voice nova
```

### Troubleshooting Windows

**"ffmpeg não é reconhecido como comando"**
→ O ffmpeg não está no PATH. Siga o passo 2 novamente e abra um **novo terminal** após alterar o PATH.

**"VCRUNTIME140.dll não encontrado"**
→ Instale o [Visual C++ Redistributable](https://aka.ms/vs/17/release/vc_redist.x64.exe).

**"API key not found"**
→ Verifique se o `.env` está na **mesma pasta** de onde você está executando o comando. Use `dir .env` para confirmar que o arquivo existe no diretório atual.

**Erro de permissão ao criar arquivos de cache**
→ Execute o terminal como Administrador, ou use uma pasta onde seu usuário tenha permissão de escrita (ex: `Documentos` ou `Área de Trabalho`).

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
