//! MCP client coverage over the streamable-HTTP transport (mocked): the
//! initialize handshake, tools/resources/prompts, error handling, and the
//! agent-level MCP methods.

mod common;

use common::mock_http;
use corrosive_agents::prelude::*;
use serde_json::{json, Value};

/// A mock MCP server: parses the posted JSON-RPC request and answers it.
fn mcp_handler(body: &str) -> (u16, String) {
    let req: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let Some(id) = req.get("id").cloned() else {
        // Notification (e.g. notifications/initialized): 200 with no result.
        return (200, "{}".to_string());
    };
    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": { "name": "mock-mcp", "version": "9" },
            "capabilities": {}
        }),
        "tools/list" => json!({
            "tools": [{ "name": "echo", "description": "echoes text", "inputSchema": { "type": "object" } }]
        }),
        "tools/call" => {
            let name = req.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            if name == "boom" {
                json!({ "isError": true, "content": [{ "type": "text", "text": "kaboom" }] })
            } else {
                json!({ "isError": false, "content": [{ "type": "text", "text": "tool ok" }] })
            }
        }
        "resources/list" => json!({
            "resources": [{ "uri": "mem://a", "name": "A", "description": "d", "mimeType": "text/plain" }]
        }),
        "resources/read" => json!({ "contents": [{ "uri": "mem://a", "text": "body" }] }),
        "prompts/list" => json!({
            "prompts": [{ "name": "greet", "description": "greeting", "arguments": [] }]
        }),
        "prompts/get" => json!({
            "messages": [{ "role": "user", "content": { "type": "text", "text": "hello" } }]
        }),
        _ => json!({}),
    };
    (200, json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string())
}

#[tokio::test]
async fn mcp_http_client_full_surface() {
    let url = mock_http(|_m, _p, body| mcp_handler(body)).await;
    let config = McpServerConfig::http("mock", url).with_header("Authorization", "Bearer x");
    let client = McpClient::connect(&config).await.unwrap();

    assert_eq!(client.name(), "mock");
    assert_eq!(
        client.server_info().pointer("/serverInfo/name").unwrap(),
        "mock-mcp"
    );

    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");

    let out = client.call_tool("echo", json!({ "text": "hi" })).await.unwrap();
    assert!(out.to_string().contains("tool ok"));

    // A tool that reports isError → Err.
    let err = client.call_tool("boom", json!({})).await.unwrap_err();
    assert!(matches!(err, Error::Mcp(_)), "got: {err}");

    let resources = client.list_resources().await.unwrap();
    assert_eq!(resources[0].uri, "mem://a");
    let read = client.read_resource("mem://a").await.unwrap();
    assert!(read.to_string().contains("body"));

    let prompts = client.list_prompts().await.unwrap();
    assert_eq!(prompts[0].name, "greet");
    let got = client.get_prompt("greet", json!({})).await.unwrap();
    assert!(got.to_string().contains("hello"));

    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn mcp_http_surfaces_jsonrpc_errors() {
    let url = mock_http(|_m, _p, body| {
        let req: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let id = req.get("id").cloned();
        if method == "initialize" {
            return (
                200,
                json!({ "jsonrpc": "2.0", "id": id, "result": { "serverInfo": { "name": "x" } } })
                    .to_string(),
            );
        }
        let Some(id) = id else {
            return (200, "{}".to_string());
        };
        (
            200,
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "nope" } })
                .to_string(),
        )
    })
    .await;

    let client = McpClient::connect(&McpServerConfig::http("m", url)).await.unwrap();
    let err = client.list_tools().await.unwrap_err();
    assert!(matches!(err, Error::Mcp(_)), "got: {err}");
}

#[tokio::test]
async fn mcp_connect_requires_command_or_url() {
    let config = McpServerConfig::new("bad", "", Vec::<String>::new());
    let err = McpClient::connect(&config).await.unwrap_err();
    assert!(matches!(err, Error::Mcp(_)), "got: {err}");
}

#[tokio::test]
async fn mcp_http_reports_transport_errors() {
    let url = mock_http(|_m, _p, _b| (500, r#"{"error":"down"}"#.to_string())).await;
    let err = McpClient::connect(&McpServerConfig::http("m", url)).await.unwrap_err();
    assert!(matches!(err, Error::Mcp(_)), "got: {err}");
}

#[tokio::test]
async fn agent_connects_and_calls_mcp_servers() {
    let url = mock_http(|_m, _p, body| mcp_handler(body)).await;
    let agent = Agent::builder()
        .name("mcp-agent")
        .version("1.0.0")
        .mcp_server(McpServerConfig::http("mock", url))
        .build()
        .unwrap();

    let connected = agent.connect_mcp_servers().await.unwrap();
    assert_eq!(connected, vec!["mock".to_string()]);
    assert!(agent.mcp_client("mock").await.is_some());

    let tools = agent.mcp_tools("mock").await.unwrap();
    assert_eq!(tools[0].name, "echo");

    let out = agent.call_mcp_tool("mock", "echo", json!({ "x": 1 })).await.unwrap();
    assert!(out.to_string().contains("tool ok"));

    // A server that was never connected is an error.
    assert!(agent.mcp_tools("ghost").await.is_err());
    assert!(agent.call_mcp_tool("ghost", "x", json!({})).await.is_err());
}
