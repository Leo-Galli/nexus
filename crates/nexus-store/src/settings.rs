//! Simple key-value application settings. See `docs/CONTRACTS.md` for the
//! frozen function signatures this module implements.

use rusqlite::{OptionalExtension, params};

use crate::{Store, StoreResult};

pub async fn get(store: &Store, key: &str) -> StoreResult<Option<String>> {
    let key = key.to_string();
    store
        .with_connection(move |conn| {
            conn.query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
            .map_err(Into::into)
        })
        .await
}

pub async fn set(store: &Store, key: &str, value: &str) -> StoreResult<()> {
    let key = key.to_string();
    let value = value.to_string();
    store
        .with_connection(move |conn| {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
            Ok(())
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    #[tokio::test]
    async fn get_missing_key_returns_none() {
        let store = Store::open_in_memory().await.unwrap();
        assert_eq!(get(&store, "missing").await.unwrap(), None);
    }

    #[tokio::test]
    async fn set_then_get_round_trips() {
        let store = Store::open_in_memory().await.unwrap();
        set(&store, "theme", "dark").await.unwrap();
        assert_eq!(
            get(&store, "theme").await.unwrap(),
            Some("dark".to_string())
        );
    }

    #[tokio::test]
    async fn set_overwrites_existing_value() {
        let store = Store::open_in_memory().await.unwrap();
        set(&store, "theme", "dark").await.unwrap();
        set(&store, "theme", "light").await.unwrap();
        assert_eq!(
            get(&store, "theme").await.unwrap(),
            Some("light".to_string())
        );
    }
}
