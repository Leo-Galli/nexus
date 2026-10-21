//! Best-effort redaction of secret-looking substrings before anything is
//! written to the debug log. This is a defence-in-depth net, not a
//! replacement for never logging the secret in the first place.

use regex::Regex;
use std::sync::OnceLock;

fn patterns() -> &'static [Regex] {
    static PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            // Authorization / Bearer headers.
            Regex::new(r#"(?i)(authorization:\s*bearer\s+)([A-Za-z0-9\-_.=]+)"#).unwrap(),
            // Common provider API key prefixes (OpenAI, Anthropic, xAI, ...).
            Regex::new(r#"\b(sk-|sk-ant-|xai-)[A-Za-z0-9\-_]{10,}\b"#).unwrap(),
            // Generic "api_key": "..." / "apiKey": "..." JSON fields.
            Regex::new(r#"(?i)("api[_-]?key"\s*:\s*")([^"]+)(")"#).unwrap(),
        ]
    })
}

pub fn redact(input: &str) -> String {
    let mut out = input.to_string();
    for pattern in patterns() {
        out = pattern.replace_all(&out, |caps: &regex::Captures| {
            if caps.len() >= 3 {
                format!("{}[REDACTED]{}", &caps[1], caps.get(3).map(|m| m.as_str()).unwrap_or(""))
            } else {
                "[REDACTED]".to_string()
            }
        }).into_owned();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_bearer_token() {
        let input = "Authorization: Bearer sk-abcdef0123456789abcdef";
        let redacted = redact(input);
        assert!(!redacted.contains("abcdef0123456789"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn redacts_json_api_key_field() {
        let input = r#"{"api_key": "super-secret-value-1234"}"#;
        let redacted = redact(input);
        assert!(!redacted.contains("super-secret-value-1234"));
    }

    #[test]
    fn leaves_ordinary_text_untouched() {
        let input = "hello world, this is a normal log line";
        assert_eq!(redact(input), input);
    }
}
