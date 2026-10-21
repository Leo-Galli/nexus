//! Conversation CRUD plus a simple title search. See `docs/CONTRACTS.md`
//! for the frozen function signatures this module implements.

use rusqlite::{OptionalExtension, params};
use uuid::Uuid;

use crate::{Store, StoreResult, models::Conversation};

fn row_to_conversation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: row.get("id")?,
        title: row.get("title")?,
        folder: row.get("folder")?,
        is_favorite: row.get("is_favorite")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

pub async fn create(store: &Store, title: &str) -> StoreResult<Conversation> {
    let title = title.to_string();
    store
        .with_connection(move |conn| {
            let now = chrono::Utc::now();
            let conversation = Conversation {
                id: Uuid::new_v4().to_string(),
                title,
                folder: None,
                is_favorite: false,
                created_at: now,
                updated_at: now,
            };

            conn.execute(
                "INSERT INTO conversations (id, title, folder, is_favorite, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    conversation.id,
                    conversation.title,
                    conversation.folder,
                    conversation.is_favorite,
                    conversation.created_at,
                    conversation.updated_at,
                ],
            )?;

            Ok(conversation)
        })
        .await
}

pub async fn list(store: &Store, folder: Option<&str>) -> StoreResult<Vec<Conversation>> {
    let folder = folder.map(|f| f.to_string());
    store
        .with_connection(move |conn| {
            let mut conversations = Vec::new();
            match &folder {
                Some(folder) => {
                    let mut stmt = conn.prepare(
                        "SELECT id, title, folder, is_favorite, created_at, updated_at
                         FROM conversations WHERE folder = ?1 ORDER BY updated_at DESC",
                    )?;
                    let rows = stmt.query_map(params![folder], row_to_conversation)?;
                    for row in rows {
                        conversations.push(row?);
                    }
                }
                None => {
                    let mut stmt = conn.prepare(
                        "SELECT id, title, folder, is_favorite, created_at, updated_at
                         FROM conversations ORDER BY updated_at DESC",
                    )?;
                    let rows = stmt.query_map([], row_to_conversation)?;
                    for row in rows {
                        conversations.push(row?);
                    }
                }
            }
            Ok(conversations)
        })
        .await
}

pub async fn get(store: &Store, id: &str) -> StoreResult<Option<Conversation>> {
    let id = id.to_string();
    store
        .with_connection(move |conn| {
            conn.query_row(
                "SELECT id, title, folder, is_favorite, created_at, updated_at
                 FROM conversations WHERE id = ?1",
                params![id],
                row_to_conversation,
            )
            .optional()
            .map_err(Into::into)
        })
        .await
}

pub async fn rename(store: &Store, id: &str, title: &str) -> StoreResult<()> {
    let id = id.to_string();
    let title = title.to_string();
    store
        .with_connection(move |conn| {
            conn.execute(
                "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
                params![title, chrono::Utc::now(), id],
            )?;
            Ok(())
        })
        .await
}

pub async fn set_favorite(store: &Store, id: &str, favorite: bool) -> StoreResult<()> {
    let id = id.to_string();
    store
        .with_connection(move |conn| {
            conn.execute(
                "UPDATE conversations SET is_favorite = ?1, updated_at = ?2 WHERE id = ?3",
                params![favorite, chrono::Utc::now(), id],
            )?;
            Ok(())
        })
        .await
}

/// Deletes the conversation and, via `ON DELETE CASCADE`, every message
/// that belongs to it.
pub async fn delete(store: &Store, id: &str) -> StoreResult<()> {
    let id = id.to_string();
    store
        .with_connection(move |conn| {
            conn.execute("DELETE FROM conversations WHERE id = ?1", params![id])?;
            Ok(())
        })
        .await
}

/// Simple case-insensitive substring match against `title`.
///
/// Full-text search (SQLite FTS5) could be a future improvement once there
/// is a concrete need for it, but wiring up an FTS5 virtual table and
/// keeping it correctly in sync with `conversations` is out of scope for
/// now — this is intentionally just a `LIKE` query.
pub async fn search(store: &Store, query: &str) -> StoreResult<Vec<Conversation>> {
    let pattern = format!("%{}%", query);
    store
        .with_connection(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, title, folder, is_favorite, created_at, updated_at
                 FROM conversations WHERE LOWER(title) LIKE LOWER(?1)
                 ORDER BY updated_at DESC",
            )?;
            let rows = stmt.query_map(params![pattern], row_to_conversation)?;
            let mut conversations = Vec::new();
            for row in rows {
                conversations.push(row?);
            }
            Ok(conversations)
        })
        .await
}

pub async fn delete_all(store: &Store) -> StoreResult<()> {
    store
        .with_connection(|conn| {
            conn.execute("DELETE FROM conversations", [])?;
            Ok(())
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    #[tokio::test]
    async fn create_list_get_round_trip() {
        let store = Store::open_in_memory().await.unwrap();
        let created = create(&store, "Trip planning").await.unwrap();

        let fetched = get(&store, &created.id).await.unwrap();
        assert_eq!(fetched.map(|c| c.title), Some("Trip planning".to_string()));

        let all = list(&store, None).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, created.id);
        assert!(!all[0].is_favorite);
        assert_eq!(all[0].folder, None);
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let store = Store::open_in_memory().await.unwrap();
        assert!(get(&store, "does-not-exist").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rename_and_set_favorite_update_in_place() {
        let store = Store::open_in_memory().await.unwrap();
        let created = create(&store, "Old title").await.unwrap();

        rename(&store, &created.id, "New title").await.unwrap();
        set_favorite(&store, &created.id, true).await.unwrap();

        let fetched = get(&store, &created.id).await.unwrap().unwrap();
        assert_eq!(fetched.title, "New title");
        assert!(fetched.is_favorite);
    }

    #[tokio::test]
    async fn list_filters_by_folder() {
        let store = Store::open_in_memory().await.unwrap();
        let in_folder = create(&store, "Work chat").await.unwrap();
        let _no_folder = create(&store, "Scratch chat").await.unwrap();

        store
            .with_connection({
                let id = in_folder.id.clone();
                move |conn| {
                    conn.execute(
                        "UPDATE conversations SET folder = 'work' WHERE id = ?1",
                        rusqlite::params![id],
                    )?;
                    Ok(())
                }
            })
            .await
            .unwrap();

        let work = list(&store, Some("work")).await.unwrap();
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].id, in_folder.id);

        let all = list(&store, None).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn search_is_case_insensitive_substring_match() {
        let store = Store::open_in_memory().await.unwrap();
        create(&store, "Rust async patterns").await.unwrap();
        create(&store, "Grocery list").await.unwrap();

        let results = search(&store, "RUST").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust async patterns");

        let no_results = search(&store, "xyz").await.unwrap();
        assert!(no_results.is_empty());
    }

    #[tokio::test]
    async fn delete_removes_conversation() {
        let store = Store::open_in_memory().await.unwrap();
        let created = create(&store, "Temp").await.unwrap();

        delete(&store, &created.id).await.unwrap();
        assert!(get(&store, &created.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn delete_all_clears_every_conversation() {
        let store = Store::open_in_memory().await.unwrap();
        create(&store, "One").await.unwrap();
        create(&store, "Two").await.unwrap();

        delete_all(&store).await.unwrap();
        assert!(list(&store, None).await.unwrap().is_empty());
    }
}
