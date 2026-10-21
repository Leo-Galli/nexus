//! Secret storage for Nexus, backed by the host Secret Service (or, under
//! Flatpak sandboxing, the `org.freedesktop.portal.Secret` portal) via the
//! `oo7` crate.
//!
//! `oo7::Keyring` transparently picks the right backend for the running
//! environment: a direct D-Bus connection to `org.freedesktop.secrets` when
//! one is reachable, or the portal when it isn't (e.g. inside a Flatpak
//! sandbox that has not been given direct access to the session bus's
//! Secret Service). Nothing in this crate ever writes a secret to disk
//! itself — that is the whole point of using `oo7` instead of a plain file.

use std::collections::HashMap;

use async_trait::async_trait;
use nexus_core::secrets::SecretError;
use nexus_core::{SecretKey, SecretStore};

/// Attribute key that scopes every item Nexus stores to this application,
/// so lookups (in particular [`KeyringSecretStore::list_configured_providers`])
/// never pick up unrelated Secret Service items belonging to other apps.
const APP_ATTRIBUTE: &str = "application";
/// Attribute key holding [`SecretKey::provider`].
const PROVIDER_ATTRIBUTE: &str = "provider";
/// Attribute key holding [`SecretKey::account`].
const ACCOUNT_ATTRIBUTE: &str = "account";

/// The Flatpak application id, matching the `app-id` used in the
/// `build-aux/flatpak/*.json` manifest and the desktop/metainfo files.
const APPLICATION_ID: &str = "io.github.GITHUB_OWNER.Nexus";

/// [`SecretStore`] implementation backed by `oo7::Keyring`.
///
/// Every stored secret carries three lookup attributes — `application`,
/// `provider`, `account` — and never anything else, so other applications'
/// Secret Service items are never touched and the secret value itself is
/// never used as a lookup key.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringSecretStore;

impl KeyringSecretStore {
    pub fn new() -> Self {
        Self
    }

    /// Builds the attribute map that identifies exactly one stored secret.
    /// Factored out from the backend calls so the "what gets stored under
    /// which attributes" logic can be unit-tested without a real keyring.
    fn attributes(key: &SecretKey) -> HashMap<&'static str, String> {
        HashMap::from([
            (APP_ATTRIBUTE, APPLICATION_ID.to_string()),
            (PROVIDER_ATTRIBUTE, key.provider.clone()),
            (ACCOUNT_ATTRIBUTE, key.account.clone()),
        ])
    }

    /// Attribute map that scopes a search to "every secret belonging to
    /// this application", used by [`list_configured_providers`].
    ///
    /// [`list_configured_providers`]: KeyringSecretStore::list_configured_providers
    fn application_scope_attributes() -> HashMap<&'static str, String> {
        HashMap::from([(APP_ATTRIBUTE, APPLICATION_ID.to_string())])
    }

    /// Human-readable label shown by `seahorse`/GNOME Settings' "Passwords
    /// & Keys" view.
    fn label(key: &SecretKey) -> String {
        format!("Nexus — {} ({})", key.provider, key.account)
    }

    fn borrowed<'a>(attributes: &'a HashMap<&'static str, String>) -> HashMap<&'a str, &'a str> {
        attributes.iter().map(|(k, v)| (*k, v.as_str())).collect()
    }

    /// Opens a keyring handle. A fresh handle is opened per call rather
    /// than cached on `self`, matching `oo7`'s own guidance that `Keyring`
    /// is cheap to construct and internally manages the underlying D-Bus
    /// connection/session.
    async fn open_keyring() -> Result<oo7::Keyring, SecretError> {
        // VERIFY AGAINST CURRENT oo7 DOCS: `oo7::Keyring::new()` is the
        // documented async constructor as of oo7 0.3. It selects the
        // Secret Service D-Bus API when `org.freedesktop.secrets` is
        // reachable on the session bus, and otherwise falls back to the
        // `org.freedesktop.portal.Secret` portal (the case inside a
        // Flatpak sandbox without direct access to the real Secret
        // Service). A failure here (no D-Bus session at all, portal
        // missing/denied, keyring locked with no prompt possible, etc.)
        // means the backend is simply not usable right now.
        oo7::Keyring::new()
            .await
            .map_err(|err| SecretError::Unavailable(err.to_string()))
    }
}

#[async_trait]
impl SecretStore for KeyringSecretStore {
    async fn set(&self, key: &SecretKey, value: &str) -> Result<(), SecretError> {
        let keyring = Self::open_keyring().await?;
        let attributes = Self::attributes(key);
        let attr_refs = Self::borrowed(&attributes);

        // VERIFY AGAINST CURRENT oo7 DOCS: `create_item(label, attributes,
        // secret, replace)` creates a new Secret Service item, or — with
        // `replace: true` — overwrites an existing item that has the exact
        // same attribute set. This is what lets `set` behave as an upsert.
        keyring
            .create_item(&Self::label(key), &attr_refs, value.as_bytes(), true)
            .await
            .map_err(|err| SecretError::Backend(err.to_string()))
    }

    async fn get(&self, key: &SecretKey) -> Result<Option<String>, SecretError> {
        let keyring = Self::open_keyring().await?;
        let attributes = Self::attributes(key);
        let attr_refs = Self::borrowed(&attributes);

        // VERIFY AGAINST CURRENT oo7 DOCS: `search_items` returns every
        // item whose attribute set is a superset of the ones passed in;
        // since our attribute set already fully identifies a single
        // secret, we only ever look at the first match.
        let items = keyring
            .search_items(&attr_refs)
            .await
            .map_err(|err| SecretError::Backend(err.to_string()))?;

        let Some(item) = items.into_iter().next() else {
            return Ok(None);
        };

        // VERIFY AGAINST CURRENT oo7 DOCS: `Item::secret()` returns the
        // raw secret bytes (zeroized on drop by `oo7`). Nexus only ever
        // stores UTF-8 API keys/tokens, so decoding failure is treated as
        // a backend error rather than silently losing data.
        let secret = item
            .secret()
            .await
            .map_err(|err| SecretError::Backend(err.to_string()))?;

        let value = String::from_utf8(secret.to_vec()).map_err(|err| {
            SecretError::Backend(format!("stored secret is not valid UTF-8: {err}"))
        })?;

        Ok(Some(value))
    }

    async fn delete(&self, key: &SecretKey) -> Result<(), SecretError> {
        let keyring = Self::open_keyring().await?;
        let attributes = Self::attributes(key);
        let attr_refs = Self::borrowed(&attributes);

        // VERIFY AGAINST CURRENT oo7 DOCS: some oo7 releases expose a
        // `Keyring::delete_items(attributes)`/`Keyring::delete(attributes)`
        // convenience that removes every matching item in one call; others
        // only expose removal via `Item::delete()` after a `search_items`
        // lookup. The search-then-delete form below works across both
        // shapes and keeps the attribute-matching logic identical to
        // `get`/`set`. `delete` of an already-absent secret is treated as
        // success (idempotent), matching `SecretStore`'s contract.
        let items = keyring
            .search_items(&attr_refs)
            .await
            .map_err(|err| SecretError::Backend(err.to_string()))?;

        for item in items {
            item.delete()
                .await
                .map_err(|err| SecretError::Backend(err.to_string()))?;
        }

        Ok(())
    }

    async fn list_configured_providers(&self) -> Result<Vec<String>, SecretError> {
        let keyring = Self::open_keyring().await?;
        let attributes = Self::application_scope_attributes();
        let attr_refs = Self::borrowed(&attributes);

        let items = keyring
            .search_items(&attr_refs)
            .await
            .map_err(|err| SecretError::Backend(err.to_string()))?;

        let mut providers = Vec::new();
        for item in items {
            // VERIFY AGAINST CURRENT oo7 DOCS: `Item::attributes()` returns
            // the item's stored attribute map. This deliberately never
            // calls `Item::secret()` — this function must not read, let
            // alone return, any secret value.
            let item_attrs = item
                .attributes()
                .await
                .map_err(|err| SecretError::Backend(err.to_string()))?;

            if let Some(provider) = item_attrs.get(PROVIDER_ATTRIBUTE) {
                if !providers.contains(provider) {
                    providers.push(provider.clone());
                }
            }
        }

        Ok(providers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These exercise only the pure "what attributes/label do we use"
    // logic, so they run in any sandbox without touching a real keyring.

    #[test]
    fn attributes_scope_to_application_provider_and_account() {
        let key = SecretKey::new("openai", "work");
        let attrs = KeyringSecretStore::attributes(&key);

        assert_eq!(attrs.len(), 3);
        assert_eq!(
            attrs.get(APP_ATTRIBUTE).map(String::as_str),
            Some(APPLICATION_ID)
        );
        assert_eq!(
            attrs.get(PROVIDER_ATTRIBUTE).map(String::as_str),
            Some("openai")
        );
        assert_eq!(
            attrs.get(ACCOUNT_ATTRIBUTE).map(String::as_str),
            Some("work")
        );
    }

    #[test]
    fn default_account_is_preserved_verbatim() {
        let key = SecretKey::default_for("anthropic");
        let attrs = KeyringSecretStore::attributes(&key);
        assert_eq!(
            attrs.get(ACCOUNT_ATTRIBUTE).map(String::as_str),
            Some("default")
        );
    }

    #[test]
    fn application_scope_attributes_only_contain_application() {
        let attrs = KeyringSecretStore::application_scope_attributes();
        assert_eq!(attrs.len(), 1);
        assert_eq!(
            attrs.get(APP_ATTRIBUTE).map(String::as_str),
            Some(APPLICATION_ID)
        );
    }

    #[test]
    fn label_is_human_readable_and_identifies_provider_and_account() {
        let key = SecretKey::new("anthropic", "default");
        assert_eq!(KeyringSecretStore::label(&key), "Nexus — anthropic (default)");
    }

    /// Tests that exercise the real `oo7` <-> Secret Service / portal
    /// integration. They need a real session to talk to: either a D-Bus
    /// session bus with `org.freedesktop.secrets` registered (e.g. via
    /// `gnome-keyring-daemon --replace --components=secrets`, as used by a
    /// real GNOME session or by `secret-tool`), or — when running inside a
    /// Flatpak sandbox — a working `org.freedesktop.portal.Secret`
    /// implementation (xdg-desktop-portal + a portal backend such as
    /// xdg-desktop-portal-gnome). Neither is reliably available in a
    /// headless CI sandbox, so these are `#[ignore]`d; run them manually
    /// with `cargo test -p nexus-secrets -- --ignored` on a real desktop
    /// session, or against a mock Secret Service / portal.
    mod backend {
        use super::*;
        use nexus_core::SecretStore;

        #[tokio::test]
        #[ignore = "requires a real Secret Service / portal session"]
        async fn set_then_get_then_delete_round_trips() {
            let store = KeyringSecretStore::new();
            let key = SecretKey::new("nexus-secrets-test", "default");

            store.set(&key, "super-secret-value").await.unwrap();
            let value = store.get(&key).await.unwrap();
            assert_eq!(value.as_deref(), Some("super-secret-value"));

            store.delete(&key).await.unwrap();
            let value = store.get(&key).await.unwrap();
            assert_eq!(value, None);
        }

        #[tokio::test]
        #[ignore = "requires a real Secret Service / portal session"]
        async fn list_configured_providers_reflects_stored_secrets() {
            let store = KeyringSecretStore::new();
            let key = SecretKey::new("nexus-secrets-test-list", "default");

            store.set(&key, "value").await.unwrap();
            let providers = store.list_configured_providers().await.unwrap();
            assert!(providers.contains(&"nexus-secrets-test-list".to_string()));

            store.delete(&key).await.unwrap();
        }

        #[tokio::test]
        #[ignore = "requires a real Secret Service / portal session"]
        async fn get_missing_key_returns_none() {
            let store = KeyringSecretStore::new();
            let key = SecretKey::new("nexus-secrets-test-missing", "default");
            assert_eq!(store.get(&key).await.unwrap(), None);
        }
    }
}
