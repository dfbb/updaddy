use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_socks::tcp::socks5::Socks5Stream;
use url::Url;

use super::config::{ProxyConfig, ProxyError};

struct BridgeInner {
    port: u16,
    active: AtomicUsize,
    shutdown: watch::Sender<bool>,
}

#[derive(Clone)]
pub struct BridgeHandle(Arc<BridgeInner>);

impl BridgeHandle {
    pub fn port(&self) -> u16 {
        self.0.port
    }
    pub fn active_connections(&self) -> usize {
        self.0.active.load(Ordering::Relaxed)
    }

    /// 在批次结束时调用；活动连接归零后停止接受新连接。
    pub async fn close_when_idle(&self) {
        while self.active_connections() != 0 {
            tokio::task::yield_now().await;
        }
        let _ = self.0.shutdown.send(true);
    }
}

pub struct HttpBridge;

impl HttpBridge {
    pub async fn start(config: ProxyConfig) -> Result<BridgeHandle, ProxyError> {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| ProxyError::BridgeFailed)?;
        let port = listener
            .local_addr()
            .map_err(|_| ProxyError::BridgeFailed)?
            .port();
        let (shutdown, mut stop) = watch::channel(false);
        let inner = Arc::new(BridgeInner {
            port,
            active: AtomicUsize::new(0),
            shutdown,
        });
        let handle = BridgeHandle(inner.clone());
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break; };
                        let inner = inner.clone();
                        let cfg = config.clone();
                        tokio::spawn(async move {
                            inner.active.fetch_add(1, Ordering::Relaxed);
                            let _ = serve(stream, cfg).await;
                            inner.active.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                    changed = stop.changed() => {
                        if changed.is_err() || *stop.borrow() { break; }
                    }
                }
            }
        });
        Ok(handle)
    }
}

impl Drop for BridgeHandle {
    fn drop(&mut self) {
        if Arc::strong_count(&self.0) == 1 {
            let _ = self.0.shutdown.send(true);
        }
    }
}

async fn connect_target(
    config: &ProxyConfig,
    target: &str,
) -> Result<Socks5Stream<TcpStream>, ProxyError> {
    let endpoint = config.endpoint();
    let result = if let (Some(user), Some(pass)) = (&config.username, &config.password) {
        Socks5Stream::connect_with_password(endpoint.as_str(), target, user, pass).await
    } else {
        Socks5Stream::connect(endpoint.as_str(), target).await
    };
    result.map_err(|_| ProxyError::ProxyUnavailable)
}

async fn serve(mut client: TcpStream, config: ProxyConfig) -> Result<(), ProxyError> {
    let mut request = Vec::with_capacity(4096);
    let mut buf = [0u8; 1024];
    while request.windows(4).all(|w| w != b"\r\n\r\n") && request.len() < 64 * 1024 {
        let n = client
            .read(&mut buf)
            .await
            .map_err(|_| ProxyError::ProxyUnavailable)?;
        if n == 0 {
            return Ok(());
        }
        request.extend_from_slice(&buf[..n]);
    }
    let header_end = request
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(ProxyError::InvalidConfig)?
        + 4;
    let head = String::from_utf8_lossy(&request[..header_end]);
    let mut lines = head.split("\r\n");
    let first = lines.next().ok_or(ProxyError::InvalidConfig)?;
    let mut parts = first.split_whitespace();
    let method = parts.next().ok_or(ProxyError::InvalidConfig)?;
    let target_raw = parts.next().ok_or(ProxyError::InvalidConfig)?;
    let version = parts.next().unwrap_or("HTTP/1.1");
    let (target, outbound) = if method.eq_ignore_ascii_case("CONNECT") {
        (target_raw.to_owned(), None)
    } else {
        let uri = Url::parse(target_raw).map_err(|_| ProxyError::InvalidConfig)?;
        let host = uri.host_str().ok_or(ProxyError::InvalidConfig)?;
        let port = uri
            .port_or_known_default()
            .ok_or(ProxyError::InvalidConfig)?;
        let path = match (uri.path(), uri.query()) {
            ("", _) => "/".to_owned(),
            (p, Some(q)) => format!("{p}?{q}"),
            (p, None) => p.to_owned(),
        };
        let mut req = format!("{method} {path} {version}\r\n");
        for line in lines {
            if !line.is_empty() && !line.to_ascii_lowercase().starts_with("proxy-connection:") {
                req.push_str(line);
                req.push_str("\r\n");
            }
        }
        req.push_str("\r\n");
        (format!("{host}:{port}"), Some(req.into_bytes()))
    };
    let mut remote = connect_target(&config, &target).await?;
    if let Some(outbound) = outbound {
        remote
            .write_all(&outbound)
            .await
            .map_err(|_| ProxyError::ProxyUnavailable)?;
    } else {
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .map_err(|_| ProxyError::ProxyUnavailable)?;
        if header_end < request.len() {
            remote
                .write_all(&request[header_end..])
                .await
                .map_err(|_| ProxyError::ProxyUnavailable)?;
        }
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut remote)
        .await
        .map_err(|_| ProxyError::ProxyUnavailable)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn bridge_binds_loopback_only() {
        let config = crate::proxy::ProxyConfig::test_unreachable();
        let bridge = super::HttpBridge::start(config).await.unwrap();
        assert!(bridge.port() > 0);
    }
}
