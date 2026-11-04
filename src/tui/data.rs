//! Reading stored messages back into the shapes the TUI renders.

use anyhow::Result;
use zalo_rust::chat::ChatMessage;
use zalo_rust::clock;
use zalo_rust::message_model::MessageKind;
use zalo_rust::store::{MessageStore, StoredMessage};

use crate::ctx::DB_FILE;

/// The newest `limit` stored messages of a thread, oldest first.
pub fn load_messages(thread_id: &str, limit: usize) -> Result<Vec<ChatMessage>> {
    let store = MessageStore::open(DB_FILE)?;
    Ok(store
        .messages_for(thread_id, limit)?
        .into_iter()
        .map(stored_to_chat)
        .collect())
}

fn kind_of(msg_type: &str, is_sticker: bool) -> MessageKind {
    if is_sticker {
        return MessageKind::Sticker;
    }
    match msg_type.to_lowercase().as_str() {
        "photo" | "image" => MessageKind::Photo,
        "video" => MessageKind::Video,
        "voice" | "audio" => MessageKind::Voice,
        "file" => MessageKind::File,
        "link" => MessageKind::Link,
        "gif" => MessageKind::Gif,
        "location" => MessageKind::Location,
        "contact" => MessageKind::Contact,
        _ => MessageKind::Text,
    }
}

fn str_field(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(str::to_string)
}

/// Mirror of the conversion ctdozlite performs when it hydrates a thread from SQLite, limited to
/// the fields the TUI shows or needs for replying and reacting.
pub fn stored_to_chat(sm: StoredMessage) -> ChatMessage {
    let meta: serde_json::Value = if sm.raw_json.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_str(&sm.raw_json).unwrap_or(serde_json::Value::Null)
    };
    let sticker_id = meta
        .get("stickerId")
        .or_else(|| meta.get("id"))
        .and_then(|v| v.as_u64());
    let reactions = meta
        .get("reactions")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    ChatMessage {
        kind: kind_of(&sm.msg_type, sticker_id.is_some()),
        time: clock::message_time(sm.ts),
        sender: if sm.sender_name.is_empty() {
            sm.sender_id
        } else {
            sm.sender_name
        },
        id: sm.msg_id,
        text: sm.text,
        is_outgoing: sm.is_outgoing,
        ts: sm.ts,
        sticker_id,
        sticker_cate_id: meta
            .get("cateId")
            .or_else(|| meta.get("catId"))
            .and_then(|v| v.as_u64()),
        sticker_type: meta.get("type").and_then(|v| v.as_u64()).map(|n| n as u32),
        reply_to_id: str_field(&meta, "reply_to_id"),
        reply_to_sender: str_field(&meta, "reply_to_sender"),
        reply_to_text: str_field(&meta, "reply_to_text"),
        reactions,
        is_pinned: meta.get("is_pinned").and_then(|v| v.as_bool()).unwrap_or(false),
        is_starred: meta.get("is_starred").and_then(|v| v.as_bool()).unwrap_or(false),
        link_url: str_field(&meta, "link_url"),
        link_title: str_field(&meta, "link_title"),
        link_desc: str_field(&meta, "link_desc"),
        link_thumb: str_field(&meta, "link_thumb"),
        link_src: str_field(&meta, "link_src"),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(raw: &str, msg_type: &str) -> StoredMessage {
        StoredMessage {
            msg_id: "m1".into(),
            thread_id: "t".into(),
            sender_id: "42".into(),
            sender_name: String::new(),
            text: "xin chào".into(),
            msg_type: msg_type.into(),
            ts: 1_700_000_000_000,
            is_outgoing: false,
            raw_json: raw.into(),
        }
    }

    #[test]
    fn falls_back_to_sender_id_and_keeps_basics() {
        let m = stored_to_chat(stored("", "text"));
        assert_eq!(m.sender, "42");
        assert_eq!(m.text, "xin chào");
        assert!(!m.is_outgoing);
        assert!(m.reply_to_text.is_none());
    }

    #[test]
    fn restores_reply_reactions_and_links_from_metadata() {
        let raw = r#"{"reply_to_id":"9","reply_to_sender":"An","reply_to_text":"ok?","reactions":["👍","👍"],"link_url":"https://x.vn"}"#;
        let m = stored_to_chat(stored(raw, "text"));
        assert_eq!(m.reply_to_sender.as_deref(), Some("An"));
        assert_eq!(m.reply_to_text.as_deref(), Some("ok?"));
        assert_eq!(m.reactions.len(), 2);
        assert_eq!(m.link_url.as_deref(), Some("https://x.vn"));
    }

    #[test]
    fn sticker_metadata_sets_kind() {
        let m = stored_to_chat(stored(r#"{"stickerId":7,"cateId":3,"type":7}"#, "sticker"));
        assert!(matches!(m.kind, MessageKind::Sticker));
        assert_eq!(m.sticker_id, Some(7));
        assert_eq!(m.sticker_cate_id, Some(3));
    }

    #[test]
    fn malformed_metadata_is_ignored() {
        let m = stored_to_chat(stored("{not json", "photo"));
        assert!(matches!(m.kind, MessageKind::Photo));
    }
}
