//! NVIDIA NIM client — chat, streaming, and embeddings for Nemotron models.
//!
//! Talks to the OpenAI-compatible endpoint at
//! `https://integrate.api.nvidia.com/v1`. Free API keys are available from
//! <https://build.nvidia.com> (every model page has a "Get API Key" button).

use async_stream::try_stream;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::error::{Error, Result};
use crate::llm::types::{ChatRequest, ChatResponse, StreamChunk, Usage};
use crate::llm::{EmbeddingProvider, LlmProvider};

/// Well-known NVIDIA NIM model ids usable with a free build.nvidia.com key.
///
/// The catalog evolves; list what your key can reach with
/// `GET https://integrate.api.nvidia.com/v1/models`.
pub mod models {
    /// Nemotron 3 Ultra 550B (MoE) — flagship reasoning model.
    pub const NEMOTRON_3_ULTRA_550B: &str = "nvidia/nemotron-3-ultra-550b-a55b";
    /// Nemotron 3 Super 120B (MoE) — strong quality/speed balance.
    pub const NEMOTRON_3_SUPER_120B: &str = "nvidia/nemotron-3-super-120b-a12b";
    /// Nemotron 3 Nano 30B (MoE) — fast, efficient default.
    pub const NEMOTRON_3_NANO_30B: &str = "nvidia/nemotron-3-nano-30b-a3b";
    /// Nemotron Nano 9B v2 — lightweight hybrid-Mamba model.
    pub const NEMOTRON_NANO_9B_V2: &str = "nvidia/nvidia-nemotron-nano-9b-v2";
    /// Llama 3.1 Nemotron Ultra 253B — high-quality reasoning.
    pub const LLAMA_NEMOTRON_ULTRA_253B: &str = "nvidia/llama-3.1-nemotron-ultra-253b-v1";
    /// Llama 3.3 Nemotron Super 49B.
    pub const LLAMA_NEMOTRON_SUPER_49B: &str = "nvidia/llama-3.3-nemotron-super-49b-v1";
    /// Llama 3.1 Nemotron Nano 8B — fast and lightweight.
    pub const LLAMA_NEMOTRON_NANO_8B: &str = "nvidia/llama-3.1-nemotron-nano-8b-v1";
    /// Nemotron Mini 4B Instruct — smallest, edge-friendly.
    pub const NEMOTRON_MINI_4B: &str = "nvidia/nemotron-mini-4b-instruct";
    /// Retrieval embedding model (1024 dims); use for RAG.
    pub const EMBED_QA_E5_V5: &str = "nvidia/nv-embedqa-e5-v5";
    /// Llama Nemotron Embed 1B v2 — newer retrieval embedding model.
    pub const NEMOTRON_EMBED_1B_V2: &str = "nvidia/llama-nemotron-embed-1b-v2";
}

const DEFAULT_BASE_URL: &str = "https://integrate.api.nvidia.com/v1";
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Client for NVIDIA NIM inference endpoints (Nemotron chat + embeddings).
#[derive(Clone)]
pub struct NvidiaClient {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
    embedding_model: String,
}

impl std::fmt::Debug for NvidiaClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NvidiaClient")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("embedding_model", &self.embedding_model)
            .finish_non_exhaustive()
    }
}

impl NvidiaClient {
    /// Create a client with an explicit API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .expect("reqwest client construction cannot fail with static config");
        Self {
            http,
            api_key: api_key.into(),
            base_url: DEFAULT_BASE_URL.to_string(),
            model: models::NEMOTRON_3_NANO_30B.to_string(),
            embedding_model: models::EMBED_QA_E5_V5.to_string(),
        }
    }

    /// Create a client from the `NVIDIA_API_KEY` (or legacy `NVIDIA_KEY`)
    /// environment variable.
    pub fn from_env() -> Result<Self> {
        let key = std::env::var("NVIDIA_API_KEY")
            .or_else(|_| std::env::var("NVIDIA_KEY"))
            .map_err(|_| {
                Error::Config(
                    "set NVIDIA_API_KEY (get a free key at https://build.nvidia.com)".into(),
                )
            })?;
        Ok(Self::new(key))
    }

    /// Override the default chat model (see [`models`]).
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Override the embedding model.
    #[must_use]
    pub fn with_embedding_model(mut self, model: impl Into<String>) -> Self {
        self.embedding_model = model.into();
        self
    }

    /// Point the client at a different OpenAI-compatible base URL
    /// (e.g. a self-hosted NIM container: `http://localhost:8000/v1`).
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into().trim_end_matches('/').to_string();
        self
    }

    fn chat_body(&self, request: &ChatRequest, stream: bool) -> serde_json::Value {
        json!({
            "model": request.model.as_deref().unwrap_or(&self.model),
            "messages": request.messages,
            "temperature": request.temperature.unwrap_or(0.6),
            "top_p": request.top_p.unwrap_or(0.95),
            "max_tokens": request.max_tokens.unwrap_or(2048),
            "stream": stream,
        })
    }

    async fn post_json(&self, path: &str, body: serde_json::Value) -> Result<reqwest::Response> {
        let response = self
            .http
            .post(format!("{}{path}", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(Error::Llm(format!(
                "NVIDIA API returned {status}: {detail}"
            )));
        }
        Ok(response)
    }

    async fn embed(&self, texts: &[String], input_type: &str) -> Result<Vec<Vec<f32>>> {
        let body = json!({
            "model": self.embedding_model,
            "input": texts,
            "input_type": input_type,
            "encoding_format": "float",
        });
        let response = self.post_json("/embeddings", body).await?;
        let parsed: EmbeddingsResponse = response.json().await?;
        let mut data = parsed.data;
        data.sort_by_key(|d| d.index);
        Ok(data.into_iter().map(|d| d.embedding).collect())
    }
}

#[derive(Deserialize)]
struct ChatCompletion {
    #[serde(default)]
    model: String,
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct StreamCompletion {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: StreamDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct StreamDelta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(Deserialize)]
struct EmbeddingDatum {
    index: usize,
    embedding: Vec<f32>,
}

#[async_trait::async_trait]
impl LlmProvider for NvidiaClient {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let body = self.chat_body(&request, false);
        let response = self.post_json("/chat/completions", body).await?;
        let completion: ChatCompletion = response.json().await?;
        let choice = completion
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| Error::Llm("response contained no choices".into()))?;
        Ok(ChatResponse {
            content: choice.message.content.unwrap_or_default(),
            model: completion.model,
            usage: completion.usage,
        })
    }

    async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamChunk>>> {
        let body = self.chat_body(&request, true);
        let response = self.post_json("/chat/completions", body).await?;
        let mut bytes = response.bytes_stream();

        let stream = try_stream! {
            let mut buffer = String::new();
            let mut finished = false;
            while let Some(chunk) = bytes.next().await {
                let chunk = chunk?;
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                // SSE events are separated by blank lines; process complete lines.
                while let Some(newline) = buffer.find('\n') {
                    let line = buffer[..newline].trim().to_string();
                    buffer.drain(..=newline);
                    let Some(data) = line.strip_prefix("data:") else { continue };
                    let data = data.trim();
                    if data == "[DONE]" {
                        finished = true;
                        yield StreamChunk { delta: String::new(), done: true };
                        break;
                    }
                    let event: StreamCompletion = serde_json::from_str(data)?;
                    for choice in event.choices {
                        if let Some(content) = choice.delta.content {
                            if !content.is_empty() {
                                yield StreamChunk { delta: content, done: false };
                            }
                        }
                        if choice.finish_reason.is_some() {
                            // Terminal marker still follows as [DONE]; nothing to do.
                        }
                    }
                }
                if finished {
                    break;
                }
            }
            if !finished {
                yield StreamChunk { delta: String::new(), done: true };
            }
        };
        Ok(Box::pin(stream))
    }

    fn default_model(&self) -> &str {
        &self.model
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for NvidiaClient {
    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed(texts, "passage").await
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let mut vectors = self
            .embed(std::slice::from_ref(&text.to_string()), "query")
            .await?;
        vectors
            .pop()
            .ok_or_else(|| Error::Llm("embeddings response was empty".into()))
    }
}
