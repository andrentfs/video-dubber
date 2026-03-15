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
            chunk_duration_secs: 300, // 5 minutos por chunk
        }
    }
}
