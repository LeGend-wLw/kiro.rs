use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn event_stream_frame(headers: &[(&str, &str)], payload: &str) -> Vec<u8> {
    let mut encoded_headers = Vec::new();
    for (name, value) in headers {
        encoded_headers.push(name.len() as u8);
        encoded_headers.extend_from_slice(name.as_bytes());
        encoded_headers.push(7);
        encoded_headers.extend_from_slice(&(value.len() as u16).to_be_bytes());
        encoded_headers.extend_from_slice(value.as_bytes());
    }

    let total_length = 16 + encoded_headers.len() + payload.len();
    let mut frame = Vec::with_capacity(total_length);
    frame.extend_from_slice(&(total_length as u32).to_be_bytes());
    frame.extend_from_slice(&(encoded_headers.len() as u32).to_be_bytes());
    let prelude_crc = crate::kiro::parser::crc::crc32(&frame);
    frame.extend_from_slice(&prelude_crc.to_be_bytes());
    frame.extend_from_slice(&encoded_headers);
    frame.extend_from_slice(payload.as_bytes());
    let message_crc = crate::kiro::parser::crc::crc32(&frame);
    frame.extend_from_slice(&message_crc.to_be_bytes());
    frame
}

fn exception_frame() -> Vec<u8> {
    event_stream_frame(
        &[(":message-type", "exception"), (":exception-type", "error")],
        &format!(r#"{{"message":"{TRANSIENT_UPSTREAM_ERROR}"}}"#),
    )
}

fn assistant_frame() -> Vec<u8> {
    event_stream_frame(
        &[
            (":message-type", "event"),
            (":event-type", "assistantResponseEvent"),
        ],
        r#"{"content":"recovered"}"#,
    )
}

async fn response_server<B>(body: B) -> (String, tokio::task::JoinHandle<()>)
where
    B: Fn(usize) -> Vec<u8> + Clone + Send + Sync + 'static,
{
    let attempts = Arc::new(AtomicUsize::new(0));
    let app = axum::Router::new().route(
        "/",
        axum::routing::get(move || {
            let attempts = attempts.clone();
            let body = body.clone();
            async move {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                axum::body::Body::from(body(attempt))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener should bind");
    let address = listener
        .local_addr()
        .expect("listener should have an address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("test server should run");
    });
    (format!("http://{address}/"), server)
}

async fn collect(response: KiroStreamResponse) -> anyhow::Result<Vec<u8>> {
    let mut stream = Box::pin(response.bytes_stream());
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk?);
    }
    Ok(body)
}

#[test]
fn transient_exception_matching_is_exact() {
    // Given: exact, wrapped, and merely containing variants of the upstream message.
    let exact = Event::Exception {
        exception_type: "error".to_string(),
        message: format!(r#"{{"message":"{TRANSIENT_UPSTREAM_ERROR}"}}"#),
    };
    let longer = Event::Exception {
        exception_type: "error".to_string(),
        message: format!(r#"{{"message":"prefix {TRANSIENT_UPSTREAM_ERROR}"}}"#),
    };

    // When: stream-start retry eligibility is evaluated.
    let exact_retryable = is_retryable_start_event(&exact);
    let longer_retryable = is_retryable_start_event(&longer);

    // Then: only the protocol's exact transient message is retried.
    assert!(exact_retryable);
    assert!(!longer_retryable);
}

#[tokio::test]
async fn transient_first_event_is_retried_before_response_is_exposed() {
    // Given: the first HTTP stream contains metadata plus the exception, then a valid response.
    let attempts = Arc::new(AtomicUsize::new(0));
    let server_attempts = attempts.clone();
    let metadata = event_stream_frame(
        &[(":message-type", "event"), (":event-type", "unknown")],
        "{}",
    );
    let (url, server) = response_server(move |attempt| {
        server_attempts.fetch_add(1, Ordering::SeqCst);
        if attempt == 0 {
            [metadata.clone(), exception_frame()].concat()
        } else {
            assistant_frame()
        }
    })
    .await;
    let initial = reqwest::get(url.clone())
        .await
        .expect("initial response should arrive");
    let retry_url = url.clone();
    let response =
        KiroStreamResponse::with_retry_request(initial, move || reqwest::get(retry_url.clone()));

    // When: the downstream begins consuming the response body.
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    let actual = collect(response)
        .await
        .expect("the second stream should be exposed");
    server.abort();

    // Then: probing is lazy, retries once, and replays only the recovered bytes.
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(actual, assistant_frame());
}

#[tokio::test]
async fn repeated_transient_exception_is_exposed_after_three_attempts() {
    // Given: every upstream attempt returns the same transient exception.
    let attempts = Arc::new(AtomicUsize::new(0));
    let server_attempts = attempts.clone();
    let (url, server) = response_server(move |_| {
        server_attempts.fetch_add(1, Ordering::SeqCst);
        exception_frame()
    })
    .await;
    let initial = reqwest::get(url.clone())
        .await
        .expect("initial response should arrive");
    let retry_url = url.clone();
    let response =
        KiroStreamResponse::with_retry_request(initial, move || reqwest::get(retry_url.clone()));

    // When: the retry budget is exhausted.
    let actual = collect(response)
        .await
        .expect("the final exception should remain readable");
    server.abort();

    // Then: the third exception is surfaced without a fourth request.
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(actual, exception_frame());
}

#[tokio::test]
async fn empty_stream_is_retried_then_reported_as_an_error() {
    // Given: all upstream attempts end cleanly without any EventStream event.
    let attempts = Arc::new(AtomicUsize::new(0));
    let server_attempts = attempts.clone();
    let (url, server) = response_server(move |_| {
        server_attempts.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    })
    .await;
    let initial = reqwest::get(url.clone())
        .await
        .expect("initial response should arrive");
    let retry_url = url.clone();
    let response =
        KiroStreamResponse::with_retry_request(initial, move || reqwest::get(retry_url.clone()));

    // When: no attempt produces a terminal event.
    let error = collect(response)
        .await
        .expect_err("an empty stream must not look like success");
    server.abort();

    // Then: the retry budget is used and EOF remains visible.
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert!(error.to_string().contains("before its first event"));
}
