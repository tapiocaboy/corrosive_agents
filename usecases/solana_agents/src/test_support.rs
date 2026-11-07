//! Test-only helpers: an in-process mock JSON-RPC server so the chain code and
//! the mesh can be exercised end-to-end without touching a real network.

#![cfg(test)]

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Spawn a tiny JSON-RPC 2.0 server on an ephemeral port and return its URL.
///
/// It answers the handful of methods this use case calls with canned results,
/// dispatching on the `method` string in the request body:
///
/// - `getSlot` → `123456`
/// - `getGenesisHash` → `"MockGenesis1111…"`
/// - `getBalance` → `{ value: 2_500_000_000 }` (2.5 SOL)
/// - `requestAirdrop` → `"MockAirdropSig…"`
/// - `eth_blockNumber` → `"0x10"` (16)
/// - `eth_chainId` → `"0x1"` (1)
/// - `eth_getBalance` → `"0xde0b6b3a7640000"` (1 ETH)
/// - `forceError` → a JSON-RPC `error` object (to exercise the error path)
pub(crate) async fn spawn_mock_rpc() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock rpc");
    let addr = listener.local_addr().expect("mock rpc addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            tokio::spawn(async move {
                let req = read_http_request(&mut sock).await;
                let body = response_body(&req);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            });
        }
    });
    format!("http://127.0.0.1:{}", addr.port())
}

/// Read a full HTTP request (headers + Content-Length body).
async fn read_http_request(sock: &mut tokio::net::TcpStream) -> String {
    let mut data: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 1024];
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

fn response_body(req: &str) -> String {
    // NOTE: check EVM methods first — "eth_getBalance" contains "getBalance".
    let result = if req.contains("forceError") {
        return r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"boom"}}"#.to_string();
    } else if req.contains("eth_blockNumber") {
        "\"0x10\"".to_string()
    } else if req.contains("eth_chainId") {
        "\"0x1\"".to_string()
    } else if req.contains("eth_getBalance") {
        "\"0xde0b6b3a7640000\"".to_string()
    } else if req.contains("getSlot") {
        "123456".to_string()
    } else if req.contains("getGenesisHash") {
        "\"MockGenesis11111111111111111111111111111111\"".to_string()
    } else if req.contains("getBalance") {
        "{\"context\":{\"slot\":123456},\"value\":2500000000}".to_string()
    } else if req.contains("requestAirdrop") {
        "\"MockAirdropSig1111111111111111111111111111\"".to_string()
    } else {
        "null".to_string()
    };
    format!("{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{result}}}")
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
