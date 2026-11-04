//! Background tasks: terminal input, realtime delivery, polling, refresh and sending.
//!
//! Every task reports back through one channel of [`AppEvent`]s, so the UI loop is the only place
//! that mutates state.

use std::time::Duration;

use crossterm::event::Event;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use zalo_rust::chat::{self, ChatMessage, PollState, Thread};
use zalo_rust::session::{Session, SessionStatus};
use zalo_rust::socket;
use zalo_rust::store::{self, MessageStore};

use super::app::SendReq;
use crate::cmd::{send, sync};
use crate::ctx::DB_FILE;
use crate::threads::CLOUD_NAME;

pub enum AppEvent {
    Term(Event),
    Tick,
    /// Result of the first session check.
    Session(SessionStatus),
    /// New messages from the WebSocket or the poll fallback.
    Incoming(Vec<Thread>),
    /// Full refresh result plus non-fatal warnings.
    Refreshed(Vec<Thread>, Vec<String>),
    SendDone {
        local_id: String,
        server_id: String,
    },
    SendFailed {
        local_id: String,
        err: String,
    },
    Status(String),
    /// The server refused the credentials.
    Expired,
}

pub type Tx = UnboundedSender<AppEvent>;

/// Read terminal events on a dedicated thread (crossterm's reader blocks).
pub fn spawn_input(tx: Tx) {
    std::thread::spawn(move || {
        while let Ok(ev) = crossterm::event::read() {
            if tx.send(AppEvent::Term(ev)).is_err() {
                break;
            }
        }
    });
}

pub fn spawn_tick(tx: Tx) {
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(500));
        loop {
            iv.tick().await;
            if tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });
}

fn is_expired(e: &anyhow::Error) -> bool {
    SessionStatus::from_error(e) == SessionStatus::Expired
}

/// Seed list for `merge_previews`: the Cloud conversation, so its messages are attributed to it.
fn seed(s: &Session) -> Vec<Thread> {
    s.login
        .as_ref()
        .map(|l| l.send2me_id.clone())
        .filter(|id| !id.is_empty())
        .map(|id| {
            vec![Thread {
                id,
                name: CLOUD_NAME.into(),
                ..Default::default()
            }]
        })
        .unwrap_or_default()
}

fn persist(threads: &[Thread]) {
    if let Ok(mut st) = MessageStore::open(DB_FILE) {
        let _ = store::persist_threads(&mut st, threads);
    }
}

/// Verify the saved session, then start realtime delivery, polling and an initial refresh.
pub fn start_session(s: Session, tx: Tx) {
    tokio::spawn(async move {
        let status = s.session_status().await;
        let _ = tx.send(AppEvent::Session(status));
        if status == SessionStatus::Expired {
            return;
        }
        spawn_ws(s.clone(), tx.clone());
        spawn_poll(s.clone(), tx.clone());
        spawn_refresh(s, tx);
    });
}

fn spawn_ws(s: Session, tx: Tx) {
    tokio::spawn(async move {
        // `socket::spawn` reconnects forever with backoff and stops when the receiver is dropped.
        let mut rx = socket::spawn(s.clone());
        while let Some(ev) = rx.recv().await {
            if !ev.is_chat_ish() {
                continue;
            }
            let mut threads = seed(&s);
            chat::merge_previews(&mut threads, &ev.payload);
            threads.retain(|t| !t.messages.is_empty());
            if threads.is_empty() {
                continue;
            }
            chat::resolve_thread_names(&s, &mut threads).await;
            persist(&threads);
            if tx.send(AppEvent::Incoming(threads)).is_err() {
                break;
            }
        }
    });
}

/// HTTP long-poll safety net next to the WebSocket. Duplicates are harmless: the UI and the store
/// both de-duplicate by message ID.
fn spawn_poll(s: Session, tx: Tx) {
    tokio::spawn(async move {
        let mut state = PollState::default();
        loop {
            let mut threads = seed(&s);
            let mut failure = None;
            for _ in 0..3 {
                match chat::get_chat_from_polling(&s, &mut state).await {
                    Ok(resp) => {
                        chat::merge_previews(&mut threads, &resp);
                        let data = resp.get("data").unwrap_or(&resp);
                        let more = data.get("more").and_then(Value::as_i64).unwrap_or(0);
                        let reset = data
                            .get("resetLastActionId")
                            .and_then(Value::as_str)
                            .is_some_and(|r| r != "0" && !r.is_empty());
                        if more == 0 && !reset {
                            break;
                        }
                    }
                    Err(e) => {
                        failure = Some(e);
                        break;
                    }
                }
            }
            if let Some(e) = failure {
                if is_expired(&e) {
                    let _ = tx.send(AppEvent::Expired);
                    return;
                }
            } else {
                threads.retain(|t| !t.messages.is_empty());
                if !threads.is_empty() {
                    chat::resolve_thread_names(&s, &mut threads).await;
                    persist(&threads);
                    if tx.send(AppEvent::Incoming(threads)).is_err() {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    });
}

pub fn spawn_refresh(s: Session, tx: Tx) {
    tokio::spawn(async move {
        match sync::refresh_core(&s).await {
            Ok(r) => {
                let _ = tx.send(AppEvent::Refreshed(r.threads, r.warnings));
            }
            Err(e) if is_expired(&e) => {
                let _ = tx.send(AppEvent::Expired);
            }
            Err(e) => {
                let _ = tx.send(AppEvent::Status(format!("Đồng bộ lỗi: {e:#}")));
            }
        }
    });
}

pub fn spawn_send(s: Session, tx: Tx, req: SendReq) {
    tokio::spawn(async move {
        let res = match &req.quote {
            Some(q) => {
                chat::send_quote_message(&s, &req.thread_id, &req.text, q, req.is_group).await
            }
            None if req.is_group => chat::send_group_message(&s, &req.thread_id, &req.text).await,
            None => chat::send_message(&s, &req.thread_id, &req.text).await,
        };
        match res {
            Ok(resp) => {
                let mid = send::msg_id(&resp);
                let _ = send::persist_echo(
                    &req.thread_id,
                    &req.thread_name,
                    req.is_group,
                    &req.text,
                    &mid,
                    req.quote.as_ref(),
                );
                let _ = tx.send(AppEvent::SendDone {
                    local_id: req.local_id,
                    server_id: mid,
                });
            }
            Err(e) => {
                if is_expired(&e) {
                    let _ = tx.send(AppEvent::Expired);
                }
                let first = format!("{e:#}");
                let _ = tx.send(AppEvent::SendFailed {
                    local_id: req.local_id,
                    err: first.lines().next().unwrap_or("lỗi không rõ").to_string(),
                });
            }
        }
    });
}

pub fn spawn_react(s: Session, tx: Tx, thread_id: String, is_group: bool, msg: ChatMessage, emoji: &'static str) {
    tokio::spawn(async move {
        if let Err(e) = chat::send_reaction(&s, &thread_id, is_group, &msg, emoji).await {
            if is_expired(&e) {
                let _ = tx.send(AppEvent::Expired);
            }
            let _ = tx.send(AppEvent::Status(format!("Thả cảm xúc lỗi: {e:#}")));
        }
    });
}
