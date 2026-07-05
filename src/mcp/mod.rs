//! Model Context Protocol (MCP) support.
//!
//! Agents can declare MCP servers in their JSON manifest and connect to them
//! over stdio using JSON-RPC 2.0. The client speaks the `initialize`
//! handshake and the `tools/list` / `tools/call` methods, so any standard
//! MCP tool server (filesystem, git, fetch, …) plugs straight in.

mod client;

pub use client::McpClient;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// How to launch an MCP server, as declared in the agent manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServerConfig {
    /// Local name used to address the server (e.g. `"fs"`).
    pub name: String,
    /// Executable to spawn (e.g. `"npx"` or `"uvx"`).
    pub command: String,
    /// Arguments passed to the executable.
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables for the child process.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

impl McpServerConfig {
    /// Create a config from a name, command, and arguments.
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
        }
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
