use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Segmento de transcrição com timestamps e tradução
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    #[serde(default)]
    pub translated_text: String,
    /// Duração efetiva após ajuste de velocidade com overflow (pode ser > end_ms - start_ms)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_duration_secs: Option<f64>,
}

impl Segment {
    /// Duração do segmento em milissegundos
    pub fn duration_ms(&self) -> u64 {
        self.end_ms - self.start_ms
    }

    /// Duração em segundos (float)
    pub fn duration_secs(&self) -> f64 {
        self.duration_ms() as f64 / 1000.0
    }

    /// Caracteres por segundo do texto traduzido
    pub fn translated_chars_per_sec(&self) -> f64 {
        if self.duration_secs() <= 0.0 {
            return 0.0;
        }
        self.translated_text.len() as f64 / self.duration_secs()
    }

    /// Máximo de caracteres que cabem neste segmento a uma taxa de fala
    pub fn max_chars(&self, chars_per_sec: f64) -> usize {
        (self.duration_secs() * chars_per_sec).floor() as usize
    }
}



/// Configuração global do pipeline
#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: String,
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub voice: String,
    pub base_url: String,
    pub max_concurrent_tts: usize,
    pub chunk_duration_secs: u64,
    pub chunk_overlap_secs: u64,
    pub debug_segments: bool,
}

impl Config {
    pub fn new(
        api_key: String,
        input_path: PathBuf,
        output_path: PathBuf,
        voice: String,
    ) -> Self {
        Self {
            api_key,
            input_path,
            output_path,
            voice,
            base_url: "https://openrouter.ai/api/v1".to_string(),
            max_concurrent_tts: 5,
            chunk_duration_secs: 60, // 1 minuto por chunk (menor = timestamps mais precisos do Gemini)
            chunk_overlap_secs: 10,  // 10s de overlap entre chunks para cross-validação
            debug_segments: false,
        }
    }
}
