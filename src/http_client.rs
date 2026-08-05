//! HTTP Client 构建模块
//!
//! 提供统一的 HTTP Client 构建功能，支持代理配置

use reqwest::{Certificate, Client, Proxy};
use std::fs;
use std::time::Duration;

use crate::model::config::TlsBackend;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);
const KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// 代理配置
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ProxyConfig {
    /// 代理地址，支持 http/https/socks5
    pub url: String,
    /// 代理认证用户名
    pub username: Option<String>,
    /// 代理认证密码
    pub password: Option<String>,
}

impl ProxyConfig {
    /// 从 url 创建代理配置
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            username: None,
            password: None,
        }
    }

    /// 设置认证信息
    pub fn with_auth(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.username = Some(username.into());
        self.password = Some(password.into());
        self
    }
}

/// 构建 HTTP Client
///
/// # Arguments
/// * `proxy` - 可选的代理配置
/// * `read_timeout_secs` - 单次读取超时时间（秒）
///
/// # Returns
/// 配置好的 reqwest::Client
pub fn build_client(
    proxy: Option<&ProxyConfig>,
    read_timeout_secs: u64,
    tls_backend: TlsBackend,
    ca_cert_path: Option<&str>,
) -> anyhow::Result<Client> {
    let mut builder = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(Duration::from_secs(read_timeout_secs))
        .tcp_keepalive(Some(KEEP_ALIVE_INTERVAL))
        .http2_keep_alive_interval(KEEP_ALIVE_INTERVAL)
        .http2_keep_alive_timeout(KEEP_ALIVE_TIMEOUT)
        .http2_keep_alive_while_idle(true);

    match tls_backend {
        TlsBackend::Rustls => {
            builder = builder.use_rustls_tls();
        }
        TlsBackend::NativeTls => {
            #[cfg(feature = "native-tls")]
            {
                builder = builder.use_native_tls();
            }
            #[cfg(not(feature = "native-tls"))]
            {
                anyhow::bail!("此构建版本未包含 native-tls 后端，请在配置中改用 rustls");
            }
        }
    }

    if let Some(path) = ca_cert_path.filter(|path| !path.trim().is_empty()) {
        let cert = load_root_certificate(path)?;
        builder = builder.add_root_certificate(cert);
        tracing::debug!("HTTP Client 已加载自定义根证书: {}", path);
    }

    if let Some(proxy_config) = proxy {
        let mut proxy = Proxy::all(&proxy_config.url)?;

        // 设置代理认证
        if let (Some(username), Some(password)) = (&proxy_config.username, &proxy_config.password) {
            proxy = proxy.basic_auth(username, password);
        }

        builder = builder.proxy(proxy);
        tracing::debug!("HTTP Client 使用代理: {}", proxy_config.url);
    } else {
        // 未配置代理时必须直连，避免 reqwest 的 system-proxy 从企业 VPN/PAC
        // 或环境变量中注入另一条出站路径。
        builder = builder.no_proxy();
        tracing::debug!("HTTP Client 使用直连（已禁用系统代理）");
    }

    Ok(builder.build()?)
}

fn load_root_certificate(path: &str) -> anyhow::Result<Certificate> {
    let bytes = fs::read(path)?;
    Certificate::from_pem(&bytes)
        .or_else(|_| Certificate::from_der(&bytes))
        .map_err(|e| anyhow::anyhow!("加载根证书失败 {}: {}", path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
    async fn slow_active_response_can_outlive_configured_timeout() {
        // Given: a response that stays active but takes longer than the configured timeout overall.
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
            for _ in 0..4 {
                tokio::time::sleep(Duration::from_millis(350)).await;
                if socket.write_all(b"1\r\na\r\n").await.is_err() {
                    return;
                }
            }
            let _ = socket.write_all(b"0\r\n\r\n").await;
        });
        let client =
            build_client(None, 1, TlsBackend::Rustls, None).expect("test client should build");

        // When: the complete active body is consumed.
        let body = client
            .get(format!("http://{address}/"))
            .send()
            .await
            .expect("response headers should arrive")
            .bytes()
            .await;
        server.await.expect("test server should finish");

        // Then: elapsed total time does not abort a body that keeps making progress.
        assert_eq!(
            body.expect("active response body should not hit a total timeout"),
            "aaaa"
        );
    }
}
