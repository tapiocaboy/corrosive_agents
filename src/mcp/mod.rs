//! Model Context Protocol (MCP) support.
//!
//! Agents can declare MCP servers in their JSON manifest and connect to them
//! over two transports:
//!
//! - **stdio** — the server is spawned as a child process and spoken to over
//!   newline-delimited JSON-RPC (the classic transport).
//! - **streamable HTTP / SSE** — JSON-RPC messages are POSTed to a URL; the
//!   server answers with JSON or a `text/event-stream` body, and an
//!   `Mcp-Session-Id` header carries session affinity.
//!
//! The client speaks the `initialize` handshake plus **tools**
//! (`tools/list`, `tools/call`), **resources** (`resources/list`,
//! `resources/read`), and **prompts** (`prompts/list`, `prompts/get`).

mod client;

pub use client::McpClient;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// How to reach an MCP server, as declared in the agent manifest.
///
/// Set `command` (+ `args`/`env`) for a stdio server, **or** `url`
/// (+ `headers`) for a streamable-HTTP/SSE server. When both are present,
/// `url` wins.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct McpServerConfig {
    /// Local name used to address the server (e.g. `"fs"`).
    pub name: String,
    /// Executable to spawn for the stdio transport (e.g. `"npx"`).
    #[serde(default)]
    pub command: String,
    /// Arguments passed to the executable.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables for the child process.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Endpoint URL for the streamable-HTTP transport
    /// (e.g. `"https://mcp.example.com/mcp"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Extra HTTP headers (e.g. `Authorization`) for the HTTP transport.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
}

impl McpServerConfig {
    /// Create a stdio-transport config from a name, command, and arguments.
    pub fn new(
        name: impl Into<String>,
        command: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args: args.into_iter().map(Into::into).collect(),
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
        }
    }

    /// Create a streamable-HTTP transport config from a name and URL.
    pub fn http(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: String::new(),
            args: Vec::new(),
            env: HashMap::new(),
            url: Some(url.into()),
            headers: HashMap::new(),
        }
    }

    /// Add an HTTP header (HTTP transport only).
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

/// A tool advertised by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// Tool name (pass to [`McpClient::call_tool`]).
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// JSON Schema of the tool's arguments.
    #[serde(default, rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// A resource advertised by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    /// Resource URI (pass to [`McpClient::read_resource`]).
    pub uri: String,
    /// Human-readable name.
    #[serde(default)]
    pub name: String,
    /// Description, when provided.
    #[serde(default)]
    pub description: String,
    /// MIME type, when provided.
    #[serde(default, rename = "mimeType", skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// A prompt template advertised by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    /// Prompt name (pass to [`McpClient::get_prompt`]).
    pub name: String,
    /// Description, when provided.
    #[serde(default)]
    pub description: String,
    /// Declared arguments (name/description/required triples).
    #[serde(default)]
    pub arguments: serde_json::Value,
}
