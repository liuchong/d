//! Declarative static and delayed-SSE mock route endpoints.

use frame::BoxFuture;
use http_api::{
    HttpHandler, HttpRequest, HttpResponse, Method, StatusCode, stream_channel,
};
use std::time::Duration;

/// Single delayed SSE chunk emitted by a declarative mock route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockSseChunk {
    pub data: String,
    pub delay_ms: u64,
}

/// Declarative HTTP or SSE mock endpoint definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockRule {
    pub id: String,
    pub method: Method,
    pub path: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub sse_chunks: Vec<MockSseChunk>,
}

pub(crate) struct MockHandler {
    pub(crate) rule: MockRule,
}

impl HttpHandler for MockHandler {
    fn call(&self, _req: HttpRequest) -> BoxFuture<'static, HttpResponse> {
        let rule = self.rule.clone();
        Box::pin(async move {
            let status = StatusCode(rule.status);
            if !rule.sse_chunks.is_empty() {
                let (tx, rx) = stream_channel();
                let chunks = rule.sse_chunks;
                tokio::spawn(async move {
                    for chunk in chunks {
                        if chunk.delay_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(
                                chunk.delay_ms,
                            ))
                            .await;
                        }
                        if tx.send(chunk.data.into_bytes()).is_err() {
                            return;
                        }
                    }
                    tx.close();
                });
                let mut resp = HttpResponse::stream(
                    status,
                    "text/event-stream",
                    rx,
                    None,
                    true,
                );
                for (k, v) in rule.headers {
                    resp.headers.insert(k, v);
                }
                return resp;
            }

            let content_type = rule
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
                .map_or("application/json; charset=utf-8", |(_, v)| v.as_str())
                .to_string();
            let mut resp = HttpResponse::bytes(
                status,
                content_type,
                rule.body.into_bytes(),
            );
            for (k, v) in rule.headers {
                if !k.eq_ignore_ascii_case("content-type") {
                    resp.headers.insert(k, v);
                }
            }
            resp
        })
    }
}
