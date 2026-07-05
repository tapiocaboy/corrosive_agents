//! Chat request/response types shared by all LLM providers.

use serde::{Deserialize, Serialize};

/// Who authored a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System instructions.
    System,
    /// End-user input.
    User,
    /// Model output.
    Assistant,
    /// Tool/function result.
    Tool,
}

/// One message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Message author.
    pub role: Role,
    /// Message text.
    pub content: String,
}

impl ChatMessage {
    /// Build a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    /// Build a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    /// Build an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// Parameters for a chat completion.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The conversation so far.
    pub messages: Vec<ChatMessage>,
    /// Model id; falls back to the provider's default when `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Sampling temperature (0.0–1.0 typical).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    /// Nucleus sampling probability mass.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    /// Maximum number of tokens to generate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

impl ChatRequest {
    /// Build a request from a list of messages.
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            ..Default::default()
        }
    }

    /// Override the model id for this request.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the sampling temperature.
    #[must_use]
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Cap the number of generated tokens.
    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }
}

/// Token accounting reported by the provider.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    /// Tokens consumed by the prompt.
    #[serde(default)]
    pub prompt_tokens: u32,
    /// Tokens generated in the completion.
    #[serde(default)]
    pub completion_tokens: u32,
    /// Total tokens billed.
    #[serde(default)]
    pub total_tokens: u32,
}

/// A completed (non-streaming) chat response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    /// The assistant's reply text.
    pub content: String,
    /// The model that produced the reply.
    pub model: String,
    /// Token usage, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// One incremental piece of a streaming chat response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamChunk {
    /// New text produced since the previous chunk (may be empty on the
    /// terminal chunk).
    pub delta: String,
    /// `true` on the final chunk of the stream.
    pub done: bool,
}
