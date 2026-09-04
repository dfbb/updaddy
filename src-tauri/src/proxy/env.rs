use std::collections::HashMap;
use std::fmt;

/// 适配器声明其对代理的支持能力。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyCapability {
    Direct,
    NativeSocks5,
    HttpOnly,
    Unsupported,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProxyEnv {
    pub vars: HashMap<String, String>,
}

impl fmt::Debug for ProxyEnv {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = self
            .vars
            .keys()
            .map(|key| (key.as_str(), "<redacted>"))
            .collect::<HashMap<_, _>>();
        formatter
            .debug_struct("ProxyEnv")
            .field("vars", &redacted)
            .finish()
    }
}

impl ProxyEnv {
    pub fn new(vars: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            vars: vars
                .into_iter()
                .filter(|(key, _)| is_proxy_env_key(key))
                .collect(),
        }
    }
}

pub fn is_proxy_env_key(key: &str) -> bool {
    matches!(
        key,
        "ALL_PROXY"
            | "all_proxy"
            | "HTTP_PROXY"
            | "http_proxy"
            | "HTTPS_PROXY"
            | "https_proxy"
            | "NO_PROXY"
            | "no_proxy"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_allowlist_and_debug_redaction() {
        let env = ProxyEnv::new([
            ("ALL_PROXY".into(), "socks5://u:secret@host:1080".into()),
            ("PATH".into(), "/tmp".into()),
        ]);
        assert!(env.vars.contains_key("ALL_PROXY"));
        assert!(!env.vars.contains_key("PATH"));
        assert!(!format!("{env:?}").contains("secret"));
    }
}
