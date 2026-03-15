use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;

/// Cliente HTTP compartilhado para a API do OpenRouter
#[derive(Clone)]
pub struct OpenRouterClient {
    pub(crate) client: reqwest::Client,
    pub base_url: String,
    #[allow(dead_code)]
    pub api_key: String,
}

impl OpenRouterClient {
    pub fn new(api_key: &str, base_url: &str) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {}", api_key))
                .context("Invalid API key format")?,
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(std::time::Duration::from_secs(300)) // 5 min timeout para áudios longos
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self {
            client,
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
        })
    }

    /// Envia request para /chat/completions
    pub async fn chat_completion(&self, body: &Value) -> Result<Value> {
        let url = format!("{}/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .json(body)
            .send()
            .await
            .context("Failed to send chat completion request")?;

        let status = response.status();
        let response_text = response.text().await.context("Failed to read response")?;

        if !status.is_success() {
            anyhow::bail!(
                "OpenRouter API error ({}): {}",
                status,
                &response_text[..response_text.len().min(500)]
            );
        }

        let json: Value =
            serde_json::from_str(&response_text).context("Failed to parse API response")?;

        Ok(json)
    }
}

