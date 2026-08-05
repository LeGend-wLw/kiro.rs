#![cfg(test)]

use std::{net::SocketAddr, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinHandle,
};

use super::*;

async fn spawn_chunked_response(delays: Vec<Duration>) -> (SocketAddr, JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test listener should bind");
    let address = listener
        .local_addr()
        .expect("test listener should have an address");
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("client should connect");
        let mut request = [0_u8; 1024];
        let request_bytes = socket
            .read(&mut request)
            .await
            .expect("request should be readable");
        assert!(request_bytes > 0, "request should not be empty");
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
            .await
            .expect("response headers should be writable");
        for delay in delays {
            tokio::time::sleep(delay).await;
            if socket.write_all(b"1\r\na\r\n").await.is_err() {
                return;
            }
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
    });
    (address, server)
}

#[test]
fn test_proxy_config_new() {
    let config = ProxyConfig::new("http://127.0.0.1:7890");
    assert_eq!(config.url, "http://127.0.0.1:7890");
    assert!(config.username.is_none());
    assert!(config.password.is_none());
}

#[test]
fn test_proxy_config_with_auth() {
    let config = ProxyConfig::new("socks5://127.0.0.1:1080").with_auth("user", "pass");
    assert_eq!(config.url, "socks5://127.0.0.1:1080");
    assert_eq!(config.username, Some("user".to_string()));
    assert_eq!(config.password, Some("pass".to_string()));
}

#[test]
fn test_build_client_without_proxy() {
    let client = build_client(None, 30, TlsBackend::Rustls, None);
    assert!(client.is_ok());
}

#[test]
fn test_build_client_with_proxy() {
    let config = ProxyConfig::new("http://127.0.0.1:7890");
    let client = build_client(Some(&config), 30, TlsBackend::Rustls, None);
    assert!(client.is_ok());
}

#[tokio::test]
async fn standard_client_retains_total_timeout() {
    // Given: a response that stays active beyond the standard client's total timeout.
    let (address, server) = spawn_chunked_response(vec![Duration::from_millis(350); 4]).await;
    let client =
        build_client(None, 1, TlsBackend::Rustls, None).expect("standard test client should build");

    // When: the complete body is consumed.
    let body = tokio::time::timeout(Duration::from_secs(3), async {
        client
            .get(format!("http://{address}/"))
            .send()
            .await
            .expect("response headers should arrive")
            .bytes()
            .await
    })
    .await
    .expect("test should finish before its watchdog");
    server.await.expect("test server should finish");

    // Then: progress does not bypass the standard client's cumulative deadline.
    assert!(
        body.expect_err("standard request should time out")
            .is_timeout()
    );
}

#[tokio::test]
async fn streaming_client_allows_active_response_beyond_read_timeout() {
    // Given: a stream that makes progress within each read timeout but runs longer overall.
    let (address, server) = spawn_chunked_response(vec![Duration::from_millis(350); 4]).await;
    let client = build_streaming_client(None, 1, TlsBackend::Rustls, None)
        .expect("streaming test client should build");

    // When: the complete active body is consumed.
    let body = tokio::time::timeout(Duration::from_secs(3), async {
        client
            .get(format!("http://{address}/"))
            .send()
            .await
            .expect("response headers should arrive")
            .bytes()
            .await
    })
    .await
    .expect("test should finish before its watchdog");
    server.await.expect("test server should finish");

    // Then: elapsed total time does not abort a stream that keeps making progress.
    assert_eq!(body.expect("active stream should complete"), "aaaa");
}

#[tokio::test]
async fn streaming_client_times_out_when_body_stalls() {
    // Given: a stream that sends headers and then makes no body progress past the read timeout.
    let (address, server) = spawn_chunked_response(vec![Duration::from_millis(1500)]).await;
    let client = build_streaming_client(None, 1, TlsBackend::Rustls, None)
        .expect("streaming test client should build");

    // When: the stalled body is consumed.
    let body = tokio::time::timeout(Duration::from_secs(3), async {
        client
            .get(format!("http://{address}/"))
            .send()
            .await
            .expect("response headers should arrive")
            .bytes()
            .await
    })
    .await
    .expect("test should finish before its watchdog");
    server.await.expect("test server should finish");

    // Then: the per-read timeout still rejects a stalled response body.
    assert!(
        body.expect_err("stalled stream should time out")
            .is_timeout()
    );
}
