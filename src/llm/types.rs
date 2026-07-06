//! Chat request/response types shared by all LLM providers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

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

/// A tool invocation requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    /// Provider-assigned call id (echo it back in the tool result message).
    pub id: String,
    /// Name of the tool/skill to invoke.
    pub name: String,
    /// Parsed JSON arguments.
    pub arguments: Value,
}

/// A tool the model is allowed to call, in provider-neutral form.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Tool name (matches a registered skill name).
    pub name: String,
    /// What the tool does — shown to the model.
    pub description: String,
    /// JSON Schema of the tool's arguments.
    pub parameters: Value,
}

impl ToolSpec {
    /// Build a spec from a registered [`Skill`](crate::skills::Skill).
    pub fn from_skill(skill: &dyn crate::skills::Skill) -> Self {
        Self {
            name: skill.name().to_string(),
            description: skill.description().to_string(),
            parameters: skill.input_schema(),
        }
    }
}

/// One message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Message author.
    pub role: Role,
    /// Message text.
    pub content: String,
    /// Tool invocations attached to an assistant message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// For [`Role::Tool`] messages: the id of the call being answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    fn plain(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    /// Build a system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::plain(Role::System, content)
    }

    /// Build a user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::plain(Role::User, content)
    }

    /// Build an assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::plain(Role::Assistant, content)
    }

    /// Build an assistant message that requests tool invocations.
    pub fn assistant_tool_calls(content: impl Into<String>, calls: Vec<ToolCall>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Some(calls),
            tool_call_id: None,
        }
    }

    /// Build a tool-result message answering `tool_call_id`.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: content.into(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
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
    /// Tools the model may call (enables function calling when non-empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSpec>>,
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

    /// Offer tools to the model.
    #[must_use]
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = Some(tools);
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
    /// The assistant's reply text (may be empty when tools are called).
    pub content: String,
    /// The model that produced the reply.
    pub model: String,
    /// Tool invocations the model requested (empty when none).
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
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
