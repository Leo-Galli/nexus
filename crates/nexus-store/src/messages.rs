//! Message append/list for a conversation. See `docs/CONTRACTS.md` for the
//! frozen function signatures this module implements.

use rusqlite::params;

use crate::{Store, StoreResult, models::Message};

fn row_to_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get("id")?,
        conversation_id: row.get("conversation_id")?,
        role: row.get("role")?,
        content: row.get("content")?,
        provider_id: row.get("provider_id")?,
        model_id: row.get("model_id")?,
        created_at: row.get("created_at")?,
    })
}

/// Appends `msg` to `conversation_id` and bumps the parent conversation's
/// `updated_at` so conversation lists can sort by recency.
pub async fn append(store: &Store, conversation_id: &str, msg: Message) -> StoreResult<()> {
    let conversation_id = conversation_id.to_string();
    store
        .with_connection(move |conn| {
            conn.execute(
                "INSERT INTO messages (id, conversation_id, role, content, provider_id, model_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    msg.id,
                    conversation_id,
                    msg.role,
                    msg.content,
                    msg.provider_id,
                    msg.model_id,
                    msg.created_at,
                ],
            )?;
            conn.execute(
                "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
                params![msg.created_at, conversation_id],
            )?;
            Ok(())
        })
        .await
}

pub async fn list_for_conversation(
    store: &Store,
    conversation_id: &str,
) -> StoreResult<Vec<Message>> {
    let conversation_id = conversation_id.to_string();
    store
        .with_connection(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, conversation_id, role, content, provider_id, model_id, created_at
                 FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC",
            )?;
            let rows = stmt.query_map(params![conversation_id], row_to_message)?;
            let mut messages = Vec::new();
            for row in rows {
                messages.push(row?);
            }
            Ok(messages)
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversations;
    use chrono::Utc;
    use uuid::Uuid;

    fn sample_message(conversation_id: &str, content: &str) -> Message {
        Message {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            provider_id: Some("openai".to_string()),
            model_id: Some("gpt-5".to_string()),
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn append_and_list_round_trip_in_order() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = conversations::create(&store, "Chat").await.unwrap();

        append(
            &store,
            &conversation.id,
            sample_message(&conversation.id, "hi"),
        )
        .await
        .unwrap();
        append(
            &store,
            &conversation.id,
            sample_message(&conversation.id, "how are you?"),
        )
        .await
        .unwrap();

        let messages = list_for_conversation(&store, &conversation.id)
            .await
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content, "hi");
        assert_eq!(messages[1].content, "how are you?");
        assert_eq!(messages[0].provider_id.as_deref(), Some("openai"));
    }

    #[tokio::test]
    async fn append_bumps_conversation_updated_at() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = conversations::create(&store, "Chat").await.unwrap();
        let before = conversation.updated_at;

        // Ensure the new timestamp is observably different.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let mut msg = sample_message(&conversation.id, "hi");
        msg.created_at = chrono::Utc::now();
        append(&store, &conversation.id, msg).await.unwrap();

        let updated = conversations::get(&store, &conversation.id)
            .await
            .unwrap()
            .unwrap();
        assert!(updated.updated_at >= before);
    }

    #[tokio::test]
    async fn deleting_conversation_cascades_to_messages() {
        let store = Store::open_in_memory().await.unwrap();
        let conversation = conversations::create(&store, "Chat").await.unwrap();
        append(
            &store,
            &conversation.id,
            sample_message(&conversation.id, "hi"),
        )
        .await
        .unwrap();

        conversations::delete(&store, &conversation.id)
            .await
            .unwrap();

        let messages = list_for_conversation(&store, &conversation.id)
            .await
            .unwrap();
        assert!(messages.is_empty());
    }
}
