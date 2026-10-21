//! `BedrockProvider`: Amazon Bedrock — **STUB, NOT A WORKING INTEGRATION**.
//!
//! Do not mistake this module for a real implementation. Amazon Bedrock's
//! runtime API does not accept a simple API key: every request must be
//! signed with AWS Signature Version 4 (SigV4), which in turn needs AWS
//! credentials (an access key + secret key + optional session token, or an
//! instance role / SSO profile) rather than a single pasted secret. SigV4
//! signing is a non-trivial, security-sensitive piece of code (canonical
//! request construction, credential scoping, clock-skew handling, request
//! streaming interaction with the signature, etc.) that this workspace
//! does not currently pull in — there is no `aws-sigv4` or
//! `aws-sdk-bedrockruntime` crate (or equivalent) in `Cargo.toml`.
//!
//! Implementing this with a plain `Authorization: Bearer <key>` header, the
//! way every other provider in this crate works, would silently fail
//! against real AWS endpoints (SigV4 is mandatory, not optional) while
//! *looking* like a working provider to anyone skimming the code. That
//! would be worse than not implementing it at all, so this module instead
//! fails loudly and explicitly everywhere it is used: `status()` reports
//! `configured: false` with an explanatory `detail`, and `list_models` /
//! `send_chat` both return `ProviderError::Unsupported`.
//!
//! See `docs/LIMITATIONS.md` for the project-level tracking note on this
//! gap. Implementing real Bedrock support requires adding a signing
//! dependency (a maintained `aws-sigv4` crate, or the heavier
//! `aws-sdk-bedrockruntime`) and a credential-resolution story (env vars,
//! `~/.aws/credentials`, SSO, instance metadata) — out of scope for this
//! module.

use async_trait::async_trait;

use nexus_core::{
    AuthMethod, ChatRequest, ChatStream, ModelInfo, Provider, ProviderError, ProviderKind,
    ProviderResult, ProviderStatus,
};

const UNSUPPORTED_DETAIL: &str = "Amazon Bedrock integration is not implemented yet: it requires \
AWS SigV4 signing (not a simple API key) and a dedicated crate such as a maintained \
`aws-sigv4`/`aws-sdk-bedrockruntime`. See docs/LIMITATIONS.md.";

/// Explicit stub — see the module-level doc comment for why this does not
/// (and currently cannot, without a new dependency) make real requests.
pub struct BedrockProvider;

impl BedrockProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for BedrockProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for BedrockProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::AmazonBedrock
    }

    fn display_name(&self) -> &'static str {
        "Amazon Bedrock"
    }

    fn auth_method(&self) -> AuthMethod {
        // Not actually a bearer-token `ApiKey` in the sense every other
        // provider uses it (real Bedrock needs SigV4-signed AWS
        // credentials), but `AuthMethod` has no "AWS SigV4" variant and
        // this provider never reaches the point of sending a credential
        // anywhere, so this is a nominal placeholder, not a claim that a
        // pasted API key would work.
        AuthMethod::ApiKey
    }

    fn api_key_url(&self) -> Option<&'static str> {
        None
    }

    async fn status(&self) -> ProviderResult<ProviderStatus> {
        Ok(ProviderStatus {
            configured: false,
            reachable: Some(false),
            account_label: None,
            detail: Some(
                "Amazon Bedrock support requires AWS SigV4 request signing and is not yet \
                 implemented."
                    .into(),
            ),
        })
    }

    async fn list_models(&self) -> ProviderResult<Vec<ModelInfo>> {
        Err(ProviderError::Unsupported(UNSUPPORTED_DETAIL.into()))
    }

    async fn send_chat(&self, _request: ChatRequest) -> ProviderResult<ChatStream> {
        Err(ProviderError::Unsupported(UNSUPPORTED_DETAIL.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn status_reports_not_configured_with_explanation() {
        let provider = BedrockProvider::new();
        let status = provider
            .status()
            .await
            .expect("status() itself must not error");
        assert!(!status.configured);
        assert!(status.detail.is_some());
    }

    #[tokio::test]
    async fn list_models_is_explicitly_unsupported_not_panicking() {
        let provider = BedrockProvider::new();
        let result = provider.list_models().await;
        assert!(matches!(result, Err(ProviderError::Unsupported(_))));
    }

    #[tokio::test]
    async fn send_chat_is_explicitly_unsupported_not_panicking() {
        let provider = BedrockProvider::new();
        let request = ChatRequest {
            model_id: "anthropic.claude-3-sonnet".into(),
            messages: vec![nexus_core::ChatMessage::user("hi")],
            system_prompt: None,
            temperature: None,
            max_tokens: None,
            tools: Vec::new(),
            stream: true,
        };
        let result = provider.send_chat(request).await;
        assert!(matches!(result, Err(ProviderError::Unsupported(_))));
    }
}
