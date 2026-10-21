//! Shared, process-wide application state: the SQLite store, the secret
//! store, the live provider/CLI-agent registries, and the command runner
//! used for CLI invocations. Intended to be built once at startup and
//! handed (by reference/`Rc`) to every view.
//!
//! NOT YET WIRED INTO `NexusApplication`: `application.rs` currently opens
//! the store and constructs a bare `KeyringSecretStore` itself, through
//! its own ad-hoc `OnceLock` fields, and does not yet populate any
//! provider/CLI-agent registry at all. `Services` is the intended next
//! step — replace those ad-hoc fields with a single
//! `services: OnceLock<Rc<Services>>` built via `Services::bootstrap`,
//! and thread it through to `preferences.rs` and every view in
//! `src/views/` instead of passing the store/secret-store separately.
//! This module compiles and is unit-testable in isolation today; the
//! integration itself is left for the next pass over the UI layer.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use nexus_core::{CliAgent, CommandRunner, Provider, SecretStore};
use nexus_secrets::KeyringSecretStore;
use nexus_stats::PricingTable;
use nexus_store::Store;

pub struct Services {
    pub tokio: tokio::runtime::Handle,
    pub store: Arc<Store>,
    pub secrets: Arc<dyn SecretStore>,
    pub cli_runner: Arc<dyn CommandRunner>,
    /// Keyed by `ProviderKind::as_str()` for built-in providers, or the
    /// user-chosen id for a custom OpenAI-compatible endpoint. `RefCell`
    /// because Preferences mutates this on the main thread as the user
    /// connects/disconnects services; never touched from a tokio worker
    /// thread directly (callers clone the `Arc<dyn Provider>` they need
    /// before crossing into async code).
    pub providers: RefCell<HashMap<String, Arc<dyn Provider>>>,
    pub cli_agents: RefCell<HashMap<String, Arc<dyn CliAgent>>>,
    pub pricing: PricingTable,
}

impl Services {
    /// Runs on the tokio runtime (called via `tokio::Handle::block_on` or
    /// awaited from a `glib::spawn_future_local` future bridged through a
    /// `tokio::spawn`). Opens the on-disk database, connects to the
    /// system secret store, and populates the provider/CLI-agent
    /// registries from whatever is already configured.
    pub async fn bootstrap(
        tokio: tokio::runtime::Handle,
        data_dir: PathBuf,
    ) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&data_dir)?;
        let store = Arc::new(Store::open(&data_dir.join("nexus.db")).await?);
        let secrets: Arc<dyn SecretStore> = Arc::new(KeyringSecretStore::new());
        let cli_runner = detect_command_runner();
        let pricing = PricingTable::bundled();

        let services = Self {
            tokio,
            store,
            secrets,
            cli_runner,
            providers: RefCell::new(HashMap::new()),
            cli_agents: RefCell::new(HashMap::new()),
            pricing,
        };
        services.register_builtin_cli_agents();
        services.reload_providers_from_secrets().await?;
        Ok(services)
    }

    fn register_builtin_cli_agents(&self) {
        use nexus_cli_agents::agents::{
            AiderAgent, AntigravityAgent, ClaudeCodeAgent, CodexCliAgent, CopilotCliAgent,
            CursorAgent, GeminiCliAgent, QwenCodeAgent,
        };
        let mut agents = self.cli_agents.borrow_mut();
        let builtins: Vec<Arc<dyn CliAgent>> = vec![
            Arc::new(ClaudeCodeAgent::new()),
            Arc::new(CodexCliAgent::new()),
            Arc::new(GeminiCliAgent::new()),
            Arc::new(QwenCodeAgent::new()),
            Arc::new(CursorAgent::new(false)),
            Arc::new(AntigravityAgent::new()),
            Arc::new(AiderAgent::new(false)),
            Arc::new(CopilotCliAgent::new(false)),
        ];
        for agent in builtins {
            agents.insert(agent.info().id.to_string(), agent);
        }
    }

    /// (Re)build the provider registry for every provider that currently
    /// has a stored secret, plus the always-available local servers.
    /// Called at startup and whenever Preferences changes a connection.
    pub async fn reload_providers_from_secrets(&self) -> anyhow::Result<()> {
        use nexus_core::ProviderKind;
        use nexus_providers::{AnthropicProvider, GeminiProvider, OllamaProvider, OpenAiProvider};

        let configured = self.secrets.list_configured_providers().await?;
        let mut providers = self.providers.borrow_mut();

        // Each provider's `new` takes only the shared secret store and
        // computes its own `SecretKey` internally (it knows its own
        // provider id); it returns `ProviderResult<Self>` because building
        // the underlying `reqwest::Client` can fail.
        if configured
            .iter()
            .any(|p| p == ProviderKind::OpenAi.as_str())
        {
            providers.insert(
                ProviderKind::OpenAi.as_str().to_string(),
                Arc::new(OpenAiProvider::new(self.secrets.clone())?),
            );
        }
        if configured
            .iter()
            .any(|p| p == ProviderKind::Anthropic.as_str())
        {
            providers.insert(
                ProviderKind::Anthropic.as_str().to_string(),
                Arc::new(AnthropicProvider::new(self.secrets.clone())?),
            );
        }
        if configured
            .iter()
            .any(|p| p == ProviderKind::GoogleGemini.as_str())
        {
            providers.insert(
                ProviderKind::GoogleGemini.as_str().to_string(),
                Arc::new(GeminiProvider::new(self.secrets.clone())?),
            );
        }
        // Local servers need no credential: always registered, `status()`
        // reports them unreachable rather than erroring when not running.
        providers.insert(
            ProviderKind::Ollama.as_str().to_string(),
            Arc::new(OllamaProvider::new(None)?),
        );
        Ok(())
    }
}

fn detect_command_runner() -> Arc<dyn CommandRunner> {
    use nexus_cli_agents::{FlatpakHostCommandRunner, HostCommandRunner};
    // Running inside a Flatpak sandbox is reliably signalled by the
    // presence of /.flatpak-info (written by the sandbox itself, not
    // user-controllable from inside it).
    if std::path::Path::new("/.flatpak-info").exists() {
        Arc::new(FlatpakHostCommandRunner)
    } else {
        Arc::new(HostCommandRunner)
    }
}
