use std::time::Duration;

use url::Url;

use crate::error::ProviderError;

/// Build the single `reqwest::Client` shared by every remote provider.
/// `rustls-tls-native-roots` is selected in `Cargo.toml` so TLS never
/// depends on a vendored or system OpenSSL — important both for Flatpak
/// sandboxing and for supply-chain hygiene.
pub fn build_http_client(user_agent: &str) -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(15))
        .https_only(true)
        .build()
        .map_err(ProviderError::Network)
}

/// A client for *local* servers (Ollama, LM Studio, llama.cpp), where
/// plaintext `http://127.0.0.1` / `http://localhost` is expected and
/// `https_only` must therefore be relaxed. Still refuses any non-loopback
/// plaintext target.
pub fn build_local_http_client(user_agent: &str) -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(ProviderError::Network)
}

/// Validate a user-supplied custom endpoint URL (for the generic
/// OpenAI-compatible provider, Azure OpenAI base URL, etc).
///
/// Rules:
/// - must parse as an absolute URL;
/// - scheme must be `https`, unless the host is a loopback address
///   (`localhost`, `127.0.0.1`, `::1`) in which case `http` is allowed for
///   local model servers;
/// - no embedded credentials (`user:pass@host`) — those belong in the
///   secret store, never in a URL string that may end up in logs.
pub fn validate_endpoint_url(raw: &str) -> Result<Url, ProviderError> {
    let url =
        Url::parse(raw).map_err(|e| ProviderError::InvalidConfig(format!("invalid URL: {e}")))?;

    if !url.username().is_empty() || url.password().is_some() {
        return Err(ProviderError::InvalidConfig(
            "URL must not contain embedded credentials".into(),
        ));
    }

    let is_loopback = matches!(
        url.host_str(),
        Some("localhost") | Some("127.0.0.1") | Some("::1")
    );

    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback => Ok(url),
        "http" => Err(ProviderError::InvalidConfig(
            "plain HTTP is only allowed for localhost/127.0.0.1 endpoints".into(),
        )),
        other => Err(ProviderError::InvalidConfig(format!(
            "unsupported URL scheme: {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_https() {
        assert!(validate_endpoint_url("https://api.example.com/v1").is_ok());
    }

    #[test]
    fn accepts_loopback_http() {
        assert!(validate_endpoint_url("http://127.0.0.1:11434").is_ok());
        assert!(validate_endpoint_url("http://localhost:1234/v1").is_ok());
    }

    #[test]
    fn rejects_remote_http() {
        assert!(validate_endpoint_url("http://api.example.com").is_err());
    }

    #[test]
    fn rejects_embedded_credentials() {
        assert!(validate_endpoint_url("https://user:pass@api.example.com").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(validate_endpoint_url("not a url").is_err());
    }
}
