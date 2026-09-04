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
    let mut cursor = 0;
    while let Some(offset) = find_ascii_case_insensitive(&text[cursor..], "bearer") {
        let start = cursor + offset;
        let previous_is_word = text[..start]
            .chars()
            .next_back()
            .map(is_word_char)
            .unwrap_or(false);
        if previous_is_word {
            out.push_str(&text[cursor..start + 6]);
            cursor = start + 6;
            continue;
        }
        let after_scheme = &text[start + 6..];
        let first_after = after_scheme.chars().next();
        if first_after.map(is_word_char).unwrap_or(false) {
            out.push_str(&text[cursor..start + 6]);
            cursor = start + 6;
            continue;
        }
        let ows_len = after_scheme
            .chars()
            .take_while(|ch| matches!(ch, ' ' | '\t'))
            .map(char::len_utf8)
            .sum::<usize>();
        let separator_len = if ows_len > 0 {
            ows_len
        } else if let Some(ch) = first_after {
            if is_token_delimiter(ch) {
                out.push_str(&text[cursor..start + 6]);
                cursor = start + 6;
                continue;
            }
            ch.len_utf8()
        } else {
            out.push_str(&text[cursor..start + 6]);
            cursor = start + 6;
            continue;
        };
        let token_start = start + 6 + separator_len;
        out.push_str(&text[cursor..token_start]);
        let token = &text[token_start..];
        let token_len = token.find(is_token_delimiter).unwrap_or(token.len());
        if token_len == 0 {
            cursor = token_start;
            continue;
        }
        out.push_str("[REDACTED]");
        cursor = token_start + token_len;
    }
    out.push_str(&text[cursor..]);
    out
}

fn is_token_delimiter(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '"' | '\'' | ',' | ')' | ']' | '}')
}

fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn redact_socks_password(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some((relative, scheme)) = ["socks5h://", "socks5://"]
        .iter()
        .filter_map(|scheme| {
            find_ascii_case_insensitive(&text[cursor..], scheme).map(|offset| (offset, *scheme))
        })
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
                out.push_str(&url[..scheme.len()]);
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

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
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

    #[test]
    fn redactor_handles_case_and_optional_whitespace() {
        let text = "BEARER\tabc123 SOCKS5H://user:pw@example.test";
        let redacted = Redactor::redact(text, &[]);
        assert_eq!(
            redacted,
            "BEARER\t[REDACTED] SOCKS5H://user:[REDACTED]@example.test"
        );
    }

    #[test]
    fn redactor_handles_json_and_parenthesized_bearer_tokens() {
        let text = r#"{"auth":"Bearer abc"} (Bearer abc) NotBearer abc"#;
        let redacted = Redactor::redact(text, &[]);
        assert_eq!(
            redacted,
            r#"{"auth":"Bearer [REDACTED]"} (Bearer [REDACTED]) NotBearer abc"#
        );
    }

    #[test]
    fn redactor_rejects_bearer_words_without_a_token_boundary() {
        let text = "BearerX secret Bearerless secret Bearer:abc";
        let redacted = Redactor::redact(text, &[]);
        assert_eq!(
            redacted,
            "BearerX secret Bearerless secret Bearer:[REDACTED]"
        );
    }
}
