/// Removes credentials and caller-provided secret values from command output.
pub struct Redactor;

impl Redactor {
    pub fn redact(text: &str, secrets: &[&str]) -> String {
        let mut result = text.to_owned();
        for secret in secrets {
            if !secret.is_empty() {
                result = result.replace(secret, "[REDACTED]");
            }
        }

        result = redact_bearer(&result);
        redact_socks_password(&result)
    }
}

fn redact_bearer(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(offset) = rest.find("Bearer ") {
        out.push_str(&rest[..offset]);
        out.push_str("Bearer ");
        let token_start = &rest[offset + "Bearer ".len()..];
        let token_len = token_start
            .find(char::is_whitespace)
            .unwrap_or(token_start.len());
        out.push_str("[REDACTED]");
        rest = &token_start[token_len..];
    }
    out.push_str(rest);
    out
}

fn redact_socks_password(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some((relative, scheme)) = ["socks5://", "socks5h://"]
        .iter()
        .filter_map(|scheme| text[cursor..].find(scheme).map(|offset| (offset, *scheme)))
        .min_by_key(|(offset, _)| *offset)
    {
        let start = cursor + relative;
        out.push_str(&text[cursor..start]);
        let end = text[start..]
            .find(char::is_whitespace)
            .map(|v| start + v)
            .unwrap_or(text.len());
        let url = &text[start..end];
        if let Some(at) = url.find('@') {
            let authority = &url[scheme.len()..at];
            if let Some(colon) = authority.find(':') {
                out.push_str(scheme);
                out.push_str(&authority[..colon]);
                out.push_str(":[REDACTED]@");
                out.push_str(&url[at + 1..]);
            } else {
                out.push_str(url);
            }
        } else {
            out.push_str(url);
        }
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::Redactor;

    #[test]
    fn redactor_removes_proxy_password_and_bearer_token() {
        let text = "socks5://user:secret@example.test:1080 Bearer abc123";
        let redacted = Redactor::redact(text, &["secret", "abc123"]);
        assert!(!redacted.contains("secret"));
        assert!(!redacted.contains("abc123"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn redactor_removes_password_from_unregistered_socks_url() {
        let redacted = Redactor::redact("socks5://u:p@example.test", &[]);
        assert_eq!(redacted, "socks5://u:[REDACTED]@example.test");
    }
}
