//! MCP client over stdio (JSON-RPC, newline-delimited) or streamable
//! HTTP/SSE.

use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, RwLock};

use crate::error::{Error, Result};
use crate::mcp::{McpPrompt, McpResource, McpServerConfig, McpTool};

const PROTOCOL_VERSION: &str = "2024-11-05";

struct McpIo {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

enum Transport {
    Stdio {
        io: Mutex<McpIo>,
        child: Mutex<Child>,
    },
    Http {
        http: reqwest::Client,
        url: String,
        headers: std::collections::HashMap<String, String>,
        session_id: RwLock<Option<String>>,
    },
}

/// A connected MCP server (stdio child process or streamable-HTTP endpoint).
pub struct McpClient {
    name: String,
    transport: Transport,
    next_id: AtomicI64,
    server_info: Value,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("name", &self.name)
            .field(
                "transport",
                &match &self.transport {
                    Transport::Stdio { .. } => "stdio",
                    Transport::Http { .. } => "http",
                },
            )
            .field("server_info", &self.server_info)
            .finish_non_exhaustive()
    }
}

impl McpClient {
    /// Connect to the configured server (spawn + handshake for stdio, POST
    /// handshake for HTTP) and perform the MCP `initialize` exchange.
    pub async fn connect(config: &McpServerConfig) -> Result<Self> {
        let transport = if let Some(url) = &config.url {
            Transport::Http {
                http: reqwest::Client::new(),
                url: url.clone(),
                headers: config.headers.clone(),
                session_id: RwLock::new(None),
            }
        } else {
            if config.command.trim().is_empty() {
                return Err(Error::Mcp(format!(
                    "MCP server '{}' has neither a command nor a url",
                    config.name
                )));
            }
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
            Transport::Stdio {
                io: Mutex::new(McpIo {
                    stdin,
                    stdout: BufReader::new(stdout),
                }),
                child: Mutex::new(child),
            }
        };

        let client = Self {
            name: config.name.clone(),
            transport,
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

    // ── Tools ────────────────────────────────────────────────────────────

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

    // ── Resources ────────────────────────────────────────────────────────

    /// List the resources this server offers.
    pub async fn list_resources(&self) -> Result<Vec<McpResource>> {
        let result = self.request("resources/list", json!({})).await?;
        let resources = result
            .get("resources")
            .cloned()
            .ok_or_else(|| Error::Mcp("resources/list response missing 'resources'".into()))?;
        Ok(serde_json::from_value(resources)?)
    }

    /// Read a resource by URI; returns the `contents` array (text or blob
    /// entries).
    pub async fn read_resource(&self, uri: &str) -> Result<Value> {
        let result = self
            .request("resources/read", json!({ "uri": uri }))
            .await?;
        Ok(result.get("contents").cloned().unwrap_or(result))
    }

    // ── Prompts ──────────────────────────────────────────────────────────

    /// List the prompt templates this server offers.
    pub async fn list_prompts(&self) -> Result<Vec<McpPrompt>> {
        let result = self.request("prompts/list", json!({})).await?;
        let prompts = result
            .get("prompts")
            .cloned()
            .ok_or_else(|| Error::Mcp("prompts/list response missing 'prompts'".into()))?;
        Ok(serde_json::from_value(prompts)?)
    }

    /// Expand a prompt template with arguments; returns the rendered
    /// `messages` array.
    pub async fn get_prompt(&self, name: &str, arguments: Value) -> Result<Value> {
        let result = self
            .request(
                "prompts/get",
                json!({ "name": name, "arguments": arguments }),
            )
            .await?;
        Ok(result.get("messages").cloned().unwrap_or(result))
    }

    /// Terminate the connection (kills the child process for stdio; ends the
    /// HTTP session best-effort).
    pub async fn shutdown(&self) -> Result<()> {
        match &self.transport {
            Transport::Stdio { child, .. } => {
                let mut child = child.lock().await;
                child
                    .kill()
                    .await
                    .map_err(|e| Error::Mcp(format!("failed to kill server: {e}")))
            }
            Transport::Http {
                http,
                url,
                headers,
                session_id,
            } => {
                if let Some(sid) = session_id.read().await.clone() {
                    let mut request = http.delete(url).header("Mcp-Session-Id", sid);
                    for (name, value) in headers {
                        request = request.header(name, value);
                    }
                    let _ = request.send().await; // best-effort per spec
                }
                Ok(())
            }
        }
    }

    // ── JSON-RPC plumbing ────────────────────────────────────────────────

    async fn notify(&self, method: &str, params: Value) -> Result<()> {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        match &self.transport {
            Transport::Stdio { io, .. } => {
                let mut io = io.lock().await;
                Self::write_message(&mut io.stdin, &message).await
            }
            Transport::Http { .. } => {
                // Notifications over HTTP get a 202 with no body.
                self.http_post(&message, None).await.map(|_| ())
            }
        }
    }

    async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });

        let value = match &self.transport {
            Transport::Stdio { io, .. } => {
                let mut io = io.lock().await;
                Self::write_message(&mut io.stdin, &message).await?;
                Self::read_response_stdio(&mut io.stdout, id, method, &self.name).await?
            }
            Transport::Http { .. } => self
                .http_post(&message, Some(id))
                .await?
                .ok_or_else(|| Error::Mcp(format!("'{method}' returned no response")))?,
        };

        if let Some(error) = value.get("error") {
            return Err(Error::Mcp(format!("'{method}' failed: {error}")));
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }

    /// POST one JSON-RPC message over the streamable-HTTP transport. Returns
    /// the matching response envelope (or `None` for notifications).
    async fn http_post(&self, message: &Value, expect_id: Option<i64>) -> Result<Option<Value>> {
        let Transport::Http {
            http,
            url,
            headers,
            session_id,
        } = &self.transport
        else {
            unreachable!("http_post called on stdio transport");
        };

        let mut request = http
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(message);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if let Some(sid) = session_id.read().await.clone() {
            request = request.header("Mcp-Session-Id", sid);
        }

        let response = request
            .send()
            .await
            .map_err(|e| Error::Mcp(format!("HTTP request to '{}' failed: {e}", self.name)))?;

        let status = response.status();
        // The server assigns a session id on initialize; echo it afterwards.
        if let Some(sid) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            *session_id.write().await = Some(sid.to_string());
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(Error::Mcp(format!(
                "server '{}' returned {status}: {body}",
                self.name
            )));
        }
        let Some(expect_id) = expect_id else {
            return Ok(None); // notification — 202/200 with ignorable body
        };

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = response
            .text()
            .await
            .map_err(|e| Error::Mcp(format!("failed to read response body: {e}")))?;

        if content_type.starts_with("text/event-stream") {
            // Scan SSE events for the JSON-RPC response with our id.
            for line in body.lines() {
                let Some(data) = line.trim().strip_prefix("data:") else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(data.trim()) else {
                    continue;
                };
                if value.get("id").and_then(Value::as_i64) == Some(expect_id) {
                    return Ok(Some(value));
                }
            }
            Err(Error::Mcp(format!(
                "SSE stream from '{}' ended without a response for id {expect_id}",
                self.name
            )))
        } else {
            let value: Value = serde_json::from_str(&body)
                .map_err(|e| Error::Mcp(format!("invalid JSON from '{}': {e}", self.name)))?;
            Ok(Some(value))
        }
    }

    async fn read_response_stdio(
        stdout: &mut BufReader<ChildStdout>,
        id: i64,
        method: &str,
        name: &str,
    ) -> Result<Value> {
        // Read newline-delimited JSON until our response id shows up,
        // skipping notifications and unrelated messages.
        let mut line = String::new();
        loop {
            line.clear();
            let read = stdout
                .read_line(&mut line)
                .await
                .map_err(|e| Error::Mcp(format!("read from server failed: {e}")))?;
            if read == 0 {
                return Err(Error::Mcp(format!(
                    "server '{name}' closed the connection during '{method}'"
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
            return Ok(value);
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
