use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time;
use tokio_socks::tcp::socks5::Socks5Stream;
use url::Url;

use super::env::{ProxyCapability, ProxyEnv};
use super::http_bridge::{BridgeHandle, HttpBridge};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyMode {
    Direct,
    Socks5,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProxyConfig {
    pub mode: ProxyMode,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProxyConfig")
            .field("mode", &self.mode)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("invalid proxy configuration")]
    InvalidConfig,
    #[error("proxy capability is unsupported")]
    ProxyUnsupported,
    #[error("proxy is unavailable")]
    ProxyUnavailable,
    #[error("proxy bridge failed")]
    BridgeFailed,
}

impl ProxyConfig {
    pub fn parse(input: &str) -> Result<Self, ProxyError> {
        let url = Url::parse(input).map_err(|_| ProxyError::InvalidConfig)?;
        if url.path() != "" && url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ProxyError::InvalidConfig);
        }
        let mode = match url.scheme().to_ascii_lowercase().as_str() {
            "socks5" | "socks5h" => ProxyMode::Socks5,
            _ => return Err(ProxyError::InvalidConfig),
        };
        let host = url
            .host_str()
            .filter(|h| !h.is_empty())
            .ok_or(ProxyError::InvalidConfig)?;
        let port = url.port().ok_or(ProxyError::InvalidConfig)?;
        if port == 0 {
            return Err(ProxyError::InvalidConfig);
        }
        let username = if url.username().is_empty() {
            None
        } else {
            Some(url.username().to_owned())
        };
        let password = url.password().map(str::to_owned);
        if password.is_some() && username.is_none() {
            return Err(ProxyError::InvalidConfig);
        }
        Ok(Self {
            mode,
            host: host.to_owned(),
            port,
            username,
            password,
        })
    }

    pub(crate) fn endpoint(&self) -> String {
        format!("{}:{}", self.url_host(), self.port)
    }

    pub(crate) fn url(&self) -> String {
        let host = self.url_host();
        let mut url = format!("socks5://{host}");
        if let Some(username) = &self.username {
            let password = self.password.as_deref().unwrap_or("");
            url = format!(
                "socks5://{}:{}@{host}",
                encode_userinfo(username),
                encode_userinfo(password)
            );
        }
        format!("{}:{}", url, self.port)
    }

    fn url_host(&self) -> String {
        if self.host.starts_with('[') && self.host.ends_with(']') {
            self.host.clone()
        } else if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        }
    }

    #[cfg(test)]
    pub fn test_unreachable() -> Self {
        Self {
            mode: ProxyMode::Socks5,
            host: "127.0.0.1".into(),
            port: 1,
            username: None,
            password: None,
        }
    }
}

fn encode_userinfo(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                vec![byte as char]
            } else {
                let hex = format!("%{byte:02X}");
                hex.chars().collect()
            }
        })
        .collect()
}

#[derive(Clone)]
pub struct ProxyRuntime {
    config: ProxyConfig,
    bridge: Arc<Mutex<Option<BridgeHandle>>>,
}

impl ProxyRuntime {
    pub fn new(config: ProxyConfig) -> Self {
        Self {
            config,
            bridge: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn health_check(&self) -> Result<(), ProxyError> {
        if self.config.mode == ProxyMode::Direct {
            return Ok(());
        }
        let endpoint = self.config.endpoint();
        let result = time::timeout(Duration::from_secs(10), async {
            if let (Some(user), Some(pass)) = (&self.config.username, &self.config.password) {
                Socks5Stream::connect_with_password(
                    endpoint.as_str(),
                    "example.com:443",
                    user,
                    pass,
                )
                .await
                .map(|_| ())
            } else {
                Socks5Stream::connect(endpoint.as_str(), "example.com:443")
                    .await
                    .map(|_| ())
            }
        })
        .await
        .map_err(|_| ProxyError::ProxyUnavailable)?
        .map_err(|_| ProxyError::ProxyUnavailable);
        result
    }

    pub async fn prepare(&self, capability: ProxyCapability) -> Result<ProxyEnv, ProxyError> {
        if self.config.mode == ProxyMode::Direct {
            return Ok(ProxyEnv::default());
        }
        if matches!(
            capability,
            ProxyCapability::Direct | ProxyCapability::Unsupported
        ) {
            return Err(ProxyError::ProxyUnsupported);
        }
        self.health_check().await?;
        match capability {
            ProxyCapability::NativeSocks5 => Ok(ProxyEnv::new([(
                String::from("ALL_PROXY"),
                self.config.url(),
            )])),
            ProxyCapability::HttpOnly => {
                let mut guard = self.bridge.lock().await;
                if guard.is_none() {
                    *guard = Some(
                        HttpBridge::start(self.config.clone())
                            .await
                            .map_err(|_| ProxyError::BridgeFailed)?,
                    );
                }
                let proxy = guard.as_ref().expect("bridge inserted").proxy_url();
                Ok(ProxyEnv::new([
                    (String::from("HTTP_PROXY"), proxy.clone()),
                    (String::from("HTTPS_PROXY"), proxy),
                ]))
            }
            ProxyCapability::Unsupported | ProxyCapability::Direct => {
                Err(ProxyError::ProxyUnsupported)
            }
        }
    }
}

impl ProxyError {
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::ProxyUnavailable | Self::BridgeFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_url_parses_optional_credentials() {
        let config = ProxyConfig::parse("socks5://alice:secret@example.test:1080").unwrap();
        assert_eq!(config.host, "example.test");
        assert_eq!(config.port, 1080);
        assert_eq!(config.username.as_deref(), Some("alice"));
    }

    #[test]
    fn proxy_url_percent_encodes_credentials() {
        let config = ProxyConfig {
            mode: ProxyMode::Socks5,
            host: "example.test".into(),
            port: 1080,
            username: Some("alice@example".into()),
            password: Some("p@ss:word".into()),
        };
        assert_eq!(
            config.url(),
            "socks5://alice%40example:p%40ss%3Aword@example.test:1080"
        );
    }

    #[test]
    fn proxy_url_and_endpoint_bracket_ipv6_hosts() {
        let config = ProxyConfig::parse("socks5://[::1]:1080").unwrap();
        assert_eq!(config.endpoint(), "[::1]:1080");
        assert_eq!(config.url(), "socks5://[::1]:1080");
    }

    #[tokio::test]
    async fn enabled_proxy_never_returns_direct_fallback_when_bridge_fails() {
        let runtime = ProxyRuntime::new(ProxyConfig::test_unreachable());
        let result = runtime.prepare(ProxyCapability::HttpOnly).await;
        assert!(matches!(result, Err(ProxyError::ProxyUnavailable)));
    }

    #[tokio::test]
    async fn enabled_proxy_rejects_direct_capability() {
        let runtime = ProxyRuntime::new(ProxyConfig::test_unreachable());
        let result = runtime.prepare(ProxyCapability::Direct).await;
        assert!(matches!(result, Err(ProxyError::ProxyUnsupported)));
    }
}
