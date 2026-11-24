//! BDD/TDD tests for Layer 3 streaming-aware compression (`T-H06`).

use ext_compression::{CompressionConfig, CompressionPlugin};
use flate2::read::{GzDecoder, ZlibDecoder};
use frame::{ConfigValue, Context, FrameOptions, Spawner};
use http_api::{
    HttpRequest, HttpResponse, Method, RouteMatch, RouteSpec, RouterService,
    StatusCode, stream_channel,
};
use proto_http::{HttpEnginePlugin, fn_handler};
use std::io::Read;
use std::pin::Pin;
use std::sync::Arc;

#[derive(Default)]
struct TokioTestSpawner;

impl Spawner for TokioTestSpawner {
    fn spawn(&self, fut: frame::BoxFuture<'static, ()>) {
        tokio::spawn(fut);
    }
}

#[tokio::test]
async fn negotiates_compression_and_exempts_streaming_range_media() {
    let ctx = Context::new(FrameOptions::new(Arc::new(TokioTestSpawner)));
    ctx.plugin(HttpEnginePlugin::default(), ConfigValue::Null)
        .expect("http engine");
    ctx.plugin(
        CompressionPlugin::new(CompressionConfig::default()),
        ConfigValue::Null,
    )
    .expect("compression plugin");
    ctx.settle().await;

    let router = ctx.get::<RouterService>().expect("router").as_ref().clone();
    let sample_text = "compressible repeated text payload! ".repeat(30);
    let sample_clone = sample_text.clone();

    let _t = router
        .mount_route(
            &ctx,
            RouteSpec::new("text", RouteMatch::Exact("/text".to_string())),
            fn_handler(move |_req| {
                let s = sample_clone.clone();
                async move { HttpResponse::text(StatusCode::OK, s) }
            }),
        )
        .expect("text route");

    let _tiny = router
        .mount_route(
            &ctx,
            RouteSpec::new("tiny", RouteMatch::Exact("/tiny".to_string())),
            fn_handler(|_req| async {
                HttpResponse::text(StatusCode::OK, "hi")
            }),
        )
        .expect("tiny route");

    let _sse = router
        .mount_route(
            &ctx,
            RouteSpec::new("sse", RouteMatch::Exact("/sse".to_string())),
            fn_handler(|_req| async {
                let (tx, rx) = stream_channel();
                tokio::spawn(async move {
                    let _ = tx.send(b"data: first-event-packet-long-enough-to-test-no-compression\n\n".to_vec());
                    tokio::time::sleep(std::time::Duration::from_millis(40)).await;
                    let _ = tx.send(b"data: second-event\n\n".to_vec());
                    tx.close();
                });
                HttpResponse::stream(
                    StatusCode::OK,
                    "text/event-stream",
                    rx,
                    None,
                    true,
                )
            }),
        )
        .expect("sse route");

    let _r206 = router
        .mount_route(
            &ctx,
            RouteSpec::new(
                "partial",
                RouteMatch::Exact("/partial".to_string()),
            ),
            fn_handler(move |_req| {
                let bytes = vec![b'a'; 1024];
                async move {
                    HttpResponse::bytes(
                        StatusCode::PARTIAL_CONTENT,
                        "text/plain",
                        bytes,
                    )
                }
            }),
        )
        .expect("206 route");

    let _r304 = router
        .mount_route(
            &ctx,
            RouteSpec::new("notmod", RouteMatch::Exact("/notmod".to_string())),
            fn_handler(|_req| async {
                HttpResponse::empty(StatusCode::NOT_MODIFIED)
            }),
        )
        .expect("304 route");

    let _png = router
        .mount_route(
            &ctx,
            RouteSpec::new("png", RouteMatch::Exact("/img.png".to_string())),
            fn_handler(|_req| async {
                HttpResponse::bytes(
                    StatusCode::OK,
                    "image/png",
                    vec![1u8; 1024],
                )
            }),
        )
        .expect("png route");

    let _no_tr = router
        .mount_route(
            &ctx,
            RouteSpec::new(
                "notrans",
                RouteMatch::Exact("/notrans".to_string()),
            ),
            fn_handler(move |_req| {
                let bytes = "no transform test body ".repeat(30);
                async move {
                    HttpResponse::text(StatusCode::OK, bytes)
                        .with_header("Cache-Control", "public, no-transform")
                }
            }),
        )
        .expect("no-transform route");

    // 1. br negotiation
    let br_resp = router
        .dispatch(
            HttpRequest::new(Method::Get, "/text")
                .with_header("Accept-Encoding", "gzip, br"),
        )
        .await;
    assert_eq!(br_resp.header("content-encoding"), Some("br"));
    assert!(
        br_resp
            .header("vary")
            .is_some_and(|v| v.contains("Accept-Encoding"))
    );
    let br_bytes = br_resp.body.into_bytes().await.expect("bytes");
    let mut br_decoded = Vec::new();
    brotli::Decompressor::new(&br_bytes[..], 4096)
        .read_to_end(&mut br_decoded)
        .expect("decompress br");
    assert_eq!(String::from_utf8(br_decoded).unwrap(), sample_text);

    // 2. gzip negotiation
    let gz_resp = router
        .dispatch(
            HttpRequest::new(Method::Get, "/text")
                .with_header("Accept-Encoding", "gzip"),
        )
        .await;
    assert_eq!(gz_resp.header("content-encoding"), Some("gzip"));
    let gz_bytes = gz_resp.body.into_bytes().await.expect("bytes");
    let mut gz_decoded = Vec::new();
    GzDecoder::new(&gz_bytes[..])
        .read_to_end(&mut gz_decoded)
        .expect("decompress gzip");
    assert_eq!(String::from_utf8(gz_decoded).unwrap(), sample_text);

    // 3. deflate negotiation
    let df_resp = router
        .dispatch(
            HttpRequest::new(Method::Get, "/text")
                .with_header("Accept-Encoding", "deflate"),
        )
        .await;
    assert_eq!(df_resp.header("content-encoding"), Some("deflate"));
    let df_bytes = df_resp.body.into_bytes().await.expect("bytes");
    let mut df_decoded = Vec::new();
    ZlibDecoder::new(&df_bytes[..])
        .read_to_end(&mut df_decoded)
        .expect("decompress deflate");
    assert_eq!(String::from_utf8(df_decoded).unwrap(), sample_text);

    // 4. Exemptions: SSE streaming stays unbuffered + uncompressed
    let sse_resp = router
        .dispatch(
            HttpRequest::new(Method::Get, "/sse")
                .with_header("Accept-Encoding", "br, gzip, deflate"),
        )
        .await;
    assert!(sse_resp.header("content-encoding").is_none());
    match sse_resp.body {
        http_api::BodyKind::Stream {
            mut reader,
            streaming,
            ..
        } => {
            assert!(streaming, "SSE streaming flag must stay true");
            let first =
                std::future::poll_fn(|cx| Pin::new(&mut *reader).poll_next(cx))
                    .await
                    .expect("chunk1 some")
                    .expect("chunk1 ok");
            assert!(
                String::from_utf8_lossy(&first).contains("first-event"),
                "first chunk must arrive unbuffered"
            );
        }
        other => panic!("expected BodyKind::Stream, got {other:?}"),
    }

    // 5. Other exemptions: 206, 304, HEAD, image/png, no-transform, tiny
    for (method, path) in [
        (Method::Get, "/partial"),
        (Method::Get, "/notmod"),
        (Method::Head, "/text"),
        (Method::Get, "/img.png"),
        (Method::Get, "/notrans"),
        (Method::Get, "/tiny"),
    ] {
        let resp = router
            .dispatch(
                HttpRequest::new(method, path)
                    .with_header("Accept-Encoding", "br, gzip, deflate"),
            )
            .await;
        assert!(
            resp.header("content-encoding").is_none(),
            "{path} must be exempt from compression"
        );
    }

    ctx.dispose().await.expect("dispose");
}
