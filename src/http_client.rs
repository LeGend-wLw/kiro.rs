//! HTTP Client 构建模块
//!
//! 提供统一的 HTTP Client 构建功能，支持代理配置

use reqwest::{Certificate, Client, ClientBuilder, Proxy};
use std::fs;
use std::time::Duration;

use crate::model::config::TlsBackend;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const TCP_KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
const HTTP2_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
const HTTP2_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(20);

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
/// * `timeout_secs` - 总请求超时时间（秒）
///
/// # Returns
/// 配置好的 reqwest::Client
pub fn build_client(
    proxy: Option<&ProxyConfig>,
    timeout_secs: u64,
    tls_backend: TlsBackend,
    ca_cert_path: Option<&str>,
) -> anyhow::Result<Client> {
    Ok(client_builder(proxy, tls_backend, ca_cert_path)?
        .timeout(Duration::from_secs(timeout_secs))
        .build()?)
}

pub fn build_streaming_client(
    proxy: Option<&ProxyConfig>,
    read_timeout_secs: u64,
    tls_backend: TlsBackend,
    ca_cert_path: Option<&str>,
) -> anyhow::Result<Client> {
    Ok(client_builder(proxy, tls_backend, ca_cert_path)?
        .read_timeout(Duration::from_secs(read_timeout_secs))
        .tcp_keepalive(Some(TCP_KEEPALIVE_IDLE))
        .http2_keep_alive_interval(HTTP2_KEEPALIVE_INTERVAL)
        .http2_keep_alive_timeout(HTTP2_KEEPALIVE_TIMEOUT)
        .http2_keep_alive_while_idle(true)
        .build()?)
}

fn client_builder(
    proxy: Option<&ProxyConfig>,
    tls_backend: TlsBackend,
    ca_cert_path: Option<&str>,
) -> anyhow::Result<ClientBuilder> {
    let mut builder = Client::builder().connect_timeout(CONNECT_TIMEOUT);

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

    Ok(builder)
}

fn load_root_certificate(path: &str) -> anyhow::Result<Certificate> {
    let bytes = fs::read(path)?;
    Certificate::from_pem(&bytes)
        .or_else(|_| Certificate::from_der(&bytes))
        .map_err(|e| anyhow::anyhow!("加载根证书失败 {}: {}", path, e))
}

#[cfg(test)]
#[path = "http_client_tests.rs"]
mod tests;
