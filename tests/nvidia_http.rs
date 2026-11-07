//! NvidiaClient coverage against a mock OpenAI-compatible endpoint: chat,
//! tool-call parsing, streaming (SSE), embeddings, retry-on-429, and errors.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::mock_http;
use corrosive_agents::llm::{
    ChatMessage, ChatRequest, EmbeddingProvider, LlmProvider, NvidiaClient, RetryPolicy,
};
use futures_util::StreamExt;

#[tokio::test]
async fn chat_parses_content_tool_calls_and_usage() {
    let url = mock_http(|_m, path, _b| {
        if path.ends_with("/chat/completions") {
            (
                200,
                r#"{"model":"served-model","choices":[{"message":{"content":"hi there","tool_calls":[{"id":"c1","function":{"name":"add","arguments":"{\"a\":1,\"b\":2}"}}]}}],"usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}}"#
                    .to_string(),
            )
        } else {
            (404, "{}".to_string())
        }
    })
    .await;

    let client = NvidiaClient::new("key").with_base_url(url);
    let resp = client
        .chat(ChatRequest::new(vec![ChatMessage::user("hi")]))
        .await
        .unwrap();

    assert_eq!(resp.content, "hi there");
    assert_eq!(resp.model, "served-model");
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].name, "add");
    assert_eq!(resp.tool_calls[0].arguments["a"], 1);
    assert_eq!(resp.usage.unwrap().total_tokens, 6);
}

#[tokio::test]
async fn chat_stream_reassembles_sse_deltas() {
    let url = mock_http(|_m, path, _b| {
        if path.ends_with("/chat/completions") {
            (
                200,
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
                 data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n\
                 data: [DONE]\n\n"
                    .to_string(),
            )
        } else {
            (404, "{}".to_string())
        }
    })
    .await;

    let client = NvidiaClient::new("key").with_base_url(url);
    let mut stream = client
        .chat_stream(ChatRequest::new(vec![ChatMessage::user("hi")]))
        .await
        .unwrap();

    let mut text = String::new();
    let mut saw_done = false;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        if chunk.done {
            saw_done = true;
        } else {
            text.push_str(&chunk.delta);
        }
    }
    assert_eq!(text, "Hello");
    assert!(saw_done);
}

#[tokio::test]
async fn embeddings_sort_by_index_and_query_pops_last() {
    let url = mock_http(|_m, path, body| {
        if path.ends_with("/embeddings") {
            if body.contains("\"input_type\":\"query\"") {
                (200, r#"{"data":[{"index":0,"embedding":[0.9,0.8]}]}"#.to_string())
            } else {
                // Returned out of order to exercise the index sort.
                (
                    200,
                    r#"{"data":[{"index":1,"embedding":[0.3,0.4]},{"index":0,"embedding":[0.1,0.2]}]}"#
                        .to_string(),
                )
            }
        } else {
            (404, "{}".to_string())
        }
    })
    .await;

    let client = NvidiaClient::new("key").with_base_url(url);
    let docs = client
        .embed_documents(&["a".into(), "b".into()])
        .await
        .unwrap();
    assert_eq!(docs, vec![vec![0.1, 0.2], vec![0.3, 0.4]]);

    let query = client.embed_query("a").await.unwrap();
    assert_eq!(query, vec![0.9, 0.8]);
}

#[tokio::test]
async fn retries_on_429_then_succeeds() {
    let calls = Arc::new(AtomicUsize::new(0));
    let calls2 = Arc::clone(&calls);
    let url = mock_http(move |_m, path, _b| {
        if path.ends_with("/chat/completions") {
            let n = calls2.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                (429, r#"{"error":"rate limited"}"#.to_string())
            } else {
                (
                    200,
                    r#"{"model":"m","choices":[{"message":{"content":"recovered"}}]}"#.to_string(),
                )
            }
        } else {
            (404, "{}".to_string())
        }
    })
    .await;

    let client = NvidiaClient::new("key").with_base_url(url);
    let resp = client
        .chat(ChatRequest::new(vec![ChatMessage::user("hi")]))
        .await
        .unwrap();
    assert_eq!(resp.content, "recovered");
    assert!(calls.load(Ordering::SeqCst) >= 2, "should have retried once");
}

#[tokio::test]
async fn surfaces_server_errors_without_retry() {
    let url = mock_http(|_m, _p, _b| (500, r#"{"error":"boom"}"#.to_string())).await;
    let client = NvidiaClient::new("key")
        .with_base_url(url)
        .with_retry_policy(RetryPolicy::none());
    let err = client
        .chat(ChatRequest::new(vec![ChatMessage::user("hi")]))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("500"), "got: {err}");
}
