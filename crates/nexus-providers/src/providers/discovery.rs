//! Local-server auto-discovery for the "connect a local model server"
//! onboarding flow.
//!
//! Scope, deliberately narrow: this probes only `localhost`/`127.0.0.1` on
//! the well-known default ports of the three local-server providers this
//! crate supports (Ollama `11434`, LM Studio `1234`, llama.cpp server
//! `8080`). It does **not** scan any non-loopback address, any LAN subnet,
//! or any port beyond these fixed defaults. Probing other machines on the
//! user's network without an explicit, user-provided host/port would be a
//! privacy and security overreach for a desktop chat app — if the user
//! wants to connect to a local server on another machine, they type its
//! address into settings themselves and a provider is constructed with
//! that address directly, bypassing discovery entirely.

use std::time::Duration;

use futures_util::future::join_all;
use url::Url;

use nexus_core::ProviderKind;

/// One candidate local server this module knows how to probe, and the
/// outcome of probing it.
#[derive(Debug, Clone)]
pub struct DiscoveredLocalServer {
    pub kind: ProviderKind,
    pub base_url: Url,
    pub reachable: bool,
}

/// How long to wait for a single probe before giving up on it. Kept short
/// since every candidate is loopback-only: a real local server answers
/// near-instantly, and a closed port fails even faster, so this mostly
/// bounds the case of something firewalled-but-not-quite-closed.
const PROBE_TIMEOUT: Duration = Duration::from_millis(400);

/// The only candidates ever probed: loopback host spellings on each
/// local-server provider's well-known default port. See the module-level
/// doc comment for why this list is intentionally not extended with LAN
/// addresses or guessed alternate ports.
const DEFAULT_CANDIDATES: &[(&str, u16, ProviderKind)] = &[
    ("127.0.0.1", 11434, ProviderKind::Ollama),
    ("localhost", 11434, ProviderKind::Ollama),
    ("127.0.0.1", 1234, ProviderKind::LmStudio),
    ("localhost", 1234, ProviderKind::LmStudio),
    ("127.0.0.1", 8080, ProviderKind::LlamaCppServer),
    ("localhost", 8080, ProviderKind::LlamaCppServer),
];

/// Probe the fixed list of loopback local-server defaults concurrently and
/// report which ones answered. `client` should be built with
/// `nexus_core::http::build_local_http_client` (plain HTTP to loopback
/// must be allowed).
pub async fn discover_local_servers(client: &reqwest::Client) -> Vec<DiscoveredLocalServer> {
    discover_candidates(client, DEFAULT_CANDIDATES).await
}

/// Testable core of `discover_local_servers`, parameterized over the
/// candidate list so tests can point at a `wiremock` server on an
/// arbitrary port instead of the real hardcoded defaults.
async fn discover_candidates(
    client: &reqwest::Client,
    candidates: &[(&str, u16, ProviderKind)],
) -> Vec<DiscoveredLocalServer> {
    let probes = candidates
        .iter()
        .map(|&(host, port, kind)| probe_one(client, host, port, kind));
    join_all(probes).await
}

async fn probe_one(
    client: &reqwest::Client,
    host: &str,
    port: u16,
    kind: ProviderKind,
) -> DiscoveredLocalServer {
    // Constructed from a fixed "http" scheme plus a known-valid host
    // spelling and a `u16` port: this can never fail to parse.
    let base_url = Url::parse(&format!("http://{host}:{port}/"))
        .expect("scheme+host+port URL construction is always valid");

    let reachable = tokio::time::timeout(PROBE_TIMEOUT, client.get(base_url.clone()).send())
        .await
        .map(|result| result.is_ok())
        .unwrap_or(false);

    DiscoveredLocalServer {
        kind,
        base_url,
        reachable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::http::build_local_http_client;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn reports_reachable_true_for_a_live_server() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let addr = server
            .uri()
            .strip_prefix("http://")
            .expect("wiremock URI should be plain HTTP")
            .to_string();
        let (host, port_str) = addr.split_once(':').expect("host:port");
        let port: u16 = port_str.parse().expect("valid port");

        let client = build_local_http_client("nexus-providers-tests")
            .expect("test HTTP client should build");
        let candidates = [(host, port, ProviderKind::Ollama)];
        let results = discover_candidates(&client, &candidates).await;

        assert_eq!(results.len(), 1);
        assert!(results[0].reachable);
        assert_eq!(results[0].base_url, Url::parse(&server.uri()).unwrap().join("/").unwrap());
    }

    #[tokio::test]
    async fn reports_reachable_false_for_a_closed_port() {
        let client = build_local_http_client("nexus-providers-tests")
            .expect("test HTTP client should build");
        // Port 0 can never be connected to; this exercises the "nothing is
        // listening" path deterministically, without depending on some
        // specific port on the test machine being free.
        let candidates = [("127.0.0.1", 0u16, ProviderKind::Ollama)];
        let results = discover_candidates(&client, &candidates).await;

        assert_eq!(results.len(), 1);
        assert!(!results[0].reachable);
    }

    #[tokio::test]
    async fn default_candidates_only_target_loopback() {
        for (host, _, _) in DEFAULT_CANDIDATES {
            assert!(
                *host == "127.0.0.1" || *host == "localhost",
                "discovery must never probe a non-loopback host"
            );
        }
    }
}
