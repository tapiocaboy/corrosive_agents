//! A minimal MCP client over stdio (JSON-RPC 2.0, newline-delimited).

use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::mcp::{McpServerConfig, McpTool};

const PROTOCOL_VERSION: &str = "2024-11-05";

struct McpIo {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

/// A connected MCP server (spawned child process, stdio transport).
pub struct McpClient {
    name: String,
    io: Mutex<McpIo>,
    child: Mutex<Child>,
    next_id: AtomicI64,
    server_info: Value,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("name", &self.name)
            .field("server_info", &self.server_info)
            .finish_non_exhaustive()
    }
}

impl McpClient {
    /// Spawn the configured server process and perform the MCP `initialize`
    /// handshake.
    pub async fn connect(config: &McpServerConfig) -> Result<Self> {
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|e| Error::Mcp(format!("failed to spawn '{}': {e}", config.command)))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Mcp("child stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Mcp("child stdout unavailable".into()))?;

        let client = Self {
            name: config.name.clone(),
            io: Mutex::new(McpIo {
                stdin,
                stdout: BufReader::new(stdout),
            }),
            child: Mutex::new(child),
            next_id: AtomicI64::new(1),
            server_info: Value::Null,
        };

        let init_result = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "corrosive_agents",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }),
            )
            .await?;
        client
            .notify("notifications/initialized", json!({}))
            .await?;

        Ok(Self {
            server_info: init_result,
            ..client
        })
    }

    /// The local name of this server (from its config).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The server's `initialize` response (implementation name, version,
    /// capabilities).
    pub fn server_info(&self) -> &Value {
        &self.server_info
    }

    /// List the tools this server offers.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let result = self.request("tools/list", json!({})).await?;
        let tools = result
            .get("tools")
            .cloned()
            .ok_or_else(|| Error::Mcp("tools/list response missing 'tools'".into()))?;
        Ok(serde_json::from_value(tools)?)
    }

    /// Invoke a tool by name with JSON arguments and return its result
    /// content.
    pub async fn call_tool(&self, tool: &str, arguments: Value) -> Result<Value> {
        let result = self
            .request(
                "tools/call",
                json!({ "name": tool, "arguments": arguments }),
            )
            .await?;
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Err(Error::Mcp(format!(
                "tool '{tool}' reported an error: {result}"
            )));
        }
        Ok(result.get("content").cloned().unwrap_or(result))
    }

    /// Terminate the server process.
    pub async fn shutdown(&self) -> Result<()> {
        let mut child = self.child.lock().await;
        child
            .kill()
            .await
            .map_err(|e| Error::Mcp(format!("failed to kill server: {e}")))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let mut io = self.io.lock().await;
        Self::write_message(&mut io.stdin, &message).await
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });

        let mut io = self.io.lock().await;
        Self::write_message(&mut io.stdin, &message).await?;

        // Read newline-delimited JSON until our response id shows up,
        // skipping notifications and unrelated messages.
        let mut line = String::new();
        loop {
            line.clear();
            let read = io
                .stdout
                .read_line(&mut line)
                .await
                .map_err(|e| Error::Mcp(format!("read from server failed: {e}")))?;
            if read == 0 {
                return Err(Error::Mcp(format!(
                    "server '{}' closed the connection during '{method}'",
                    self.name
                )));
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
                continue;
            };
            if value.get("id").and_then(Value::as_i64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(Error::Mcp(format!("'{method}' failed: {error}")));
            }
            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn write_message(stdin: &mut ChildStdin, message: &Value) -> Result<()> {
        let mut payload = serde_json::to_vec(message)?;
        payload.push(b'\n');
        stdin
            .write_all(&payload)
            .await
            .map_err(|e| Error::Mcp(format!("write to server failed: {e}")))?;
        stdin
            .flush()
            .await
            .map_err(|e| Error::Mcp(format!("flush to server failed: {e}")))
    }
}
