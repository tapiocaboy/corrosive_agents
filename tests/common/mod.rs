//! Shared test helpers: a canned-response mock HTTP server and offline fake
//! LLM / embedding providers. Used by the integration tests to exercise the
//! networked and LLM-backed code paths without any real network or API key.
#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use corrosive_agents::llm::{
    ChatRequest, ChatResponse, EmbeddingProvider, LlmProvider, StreamChunk, Usage,
};
use corrosive_agents::Result;
use futures_util::stream::BoxStream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Start a mock HTTP server on an ephemeral port and return its base URL
/// (e.g. `http://127.0.0.1:54321`). `handler(method, path, body)` returns the
/// `(status, json_body)` to respond with. A `429` response automatically gets
/// a `Retry-After: 0` header so retry paths are exercised without delay.
pub async fn mock_http<F>(handler: F) -> String
where
    F: Fn(&str, &str, &str) -> (u16, String) + Send + Sync + 'static,
{
    let handler = Arc::new(handler);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let handler = Arc::clone(&handler);
            tokio::spawn(async move {
                let req = read_request(&mut sock).await;
                let request_line = req.lines().next().unwrap_or("");
                let mut parts = request_line.split(' ');
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("").to_string();
                let body = req.split("\r\n\r\n").nth(1).unwrap_or("");
                let (status, out) = handler(&method, &path, body);
                let reason = match status {
                    200 => "OK",
                    201 => "Created",
                    400 => "Bad Request",
                    401 => "Unauthorized",
                    404 => "Not Found",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    _ => "OK",
                };
                let extra = if status == 429 { "Retry-After: 0\r\n" } else { "" };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{out}",
                    out.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            });
        }
    });
    format!("http://127.0.0.1:{port}")
}

async fn read_request(sock: &mut TcpStream) -> String {
    let mut data: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        let n = match sock.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        data.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find(&data, b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&data[..pos]);
            let content_len = headers
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    if k.trim().eq_ignore_ascii_case("content-length") {
                        v.trim().parse::<usize>().ok()
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            if data.len() >= pos + 4 + content_len {
                break;
            }
        }
    }
    String::from_utf8_lossy(&data).into_owned()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// An offline LLM that echoes the last user message and reports fixed usage.
pub struct EchoLlm {
    pub model: String,
}

impl Default for EchoLlm {
    fn default() -> Self {
        Self {
            model: "echo-model".into(),
        }
    }
}

#[async_trait]
impl LlmProvider for EchoLlm {
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse> {
        let last = request
            .messages
            .last()
            .map(|m| m.content.clone())
            .unwrap_or_default();
        Ok(ChatResponse {
            content: format!("echo: {last}"),
            model: self.model.clone(),
            tool_calls: Vec::new(),
            usage: Some(Usage {
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
            }),
        })
    }

    async fn chat_stream(
        &self,
        _request: ChatRequest,
    ) -> Result<BoxStream<'static, Result<StreamChunk>>> {
        let chunks: Vec<Result<StreamChunk>> = vec![
            Ok(StreamChunk {
                delta: "hel".into(),
                done: false,
            }),
            Ok(StreamChunk {
                delta: "lo".into(),
                done: false,
            }),
            Ok(StreamChunk {
                delta: String::new(),
                done: true,
            }),
        ];
        Ok(Box::pin(futures_util::stream::iter(chunks)))
    }

    fn default_model(&self) -> &str {
        &self.model
    }
}

/// Deterministic offline embeddings: identical text → identical vector (so an
/// exact-match query ranks its own document first), different text → different.
pub struct HashEmbed {
    pub dim: usize,
}

impl Default for HashEmbed {
    fn default() -> Self {
        Self { dim: 8 }
    }
}

#[async_trait]
impl EmbeddingProvider for HashEmbed {
    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| embed_one(t, self.dim)).collect())
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(embed_one(text, self.dim))
    }
}

fn embed_one(text: &str, dim: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
    for (i, b) in text.bytes().enumerate() {
        v[i % dim] += b as f32;
    }
    v
}
