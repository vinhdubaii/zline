use std::io::{IsTerminal, Read};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use serde_json::{json, Value};
use zalo_rust::chat::{self, ChatMessage, Thread};
use zalo_rust::clock;
use zalo_rust::store::{self, MessageStore};

use crate::cli::Global;
use crate::ctx::{Ctx, DB_FILE, RELOGIN};
use crate::threads;

/// Build the message body from arguments, falling back to stdin when none (or `-`) are given.
fn compose(args: &[String], stdin: impl FnOnce() -> Result<String>) -> Result<String> {
    let text = if args.is_empty() || (args.len() == 1 && args[0] == "-") {
        stdin()?.trim_end_matches(['\n', '\r']).to_string()
    } else {
        args.join(" ")
    };
    if text.trim().is_empty() {
        bail!("Nội dung tin nhắn trống.");
    }
    Ok(text)
}

fn read_stdin() -> Result<String> {
    let mut input = std::io::stdin();
    if input.is_terminal() {
        bail!("Thiếu nội dung tin nhắn: truyền làm tham số, hoặc pipe vào stdin.");
    }
    let mut buf = String::new();
    input.read_to_string(&mut buf)?;
    Ok(buf)
}

/// Server message ID from a send response (`data.msgId`, string or number).
pub(crate) fn msg_id(resp: &Value) -> String {
    match resp.get("data").and_then(|d| d.get("msgId")) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Store the message we just sent so it shows up in `read` and the TUI without waiting for the
/// server echo. No-op when the server gave no message ID.
pub(crate) fn persist_echo(
    thread_id: &str,
    thread_name: &str,
    is_group: bool,
    text: &str,
    msg_id: &str,
    quote: Option<&ChatMessage>,
) -> Result<()> {
    if msg_id.is_empty() {
        return Ok(());
    }
    let ts = now_ms();
    let mut m = ChatMessage {
        id: msg_id.to_string(),
        text: text.to_string(),
        ts,
        time: clock::message_time(ts),
        is_outgoing: true,
        ..Default::default()
    };
    if let Some(q) = quote {
        m.reply_to_id = Some(q.id.clone());
        m.reply_to_sender = Some(q.sender.clone());
        m.reply_to_text = Some(q.text.clone());
    }
    let echo = Thread {
        id: thread_id.to_string(),
        name: thread_name.to_string(),
        is_group,
        messages: vec![m],
        ..Default::default()
    };
    store::persist_threads(&mut MessageStore::open(DB_FILE)?, &[echo])?;
    Ok(())
}

pub async fn run(g: &Global, thread: &str, text: Vec<String>) -> Result<()> {
    // Validate the body before any network access so a bad invocation fails fast.
    let text = compose(&text, read_stdin)?;

    let mut ctx = Ctx::online(g).await?;
    let mut all = threads::load_local();
    threads::ensure_cloud(&mut all, &ctx.s);
    let cloud = ctx.cloud_id().map(str::to_owned);
    let target = threads::expect_one(threads::resolve(&all, thread, cloud.as_deref()), thread)?;
    let (id, name, is_group) = (target.id.clone(), target.name.clone(), target.is_group);

    let sent = if is_group {
        chat::send_group_message(&ctx.s, &id, &text).await
    } else {
        chat::send_message(&ctx.s, &id, &text).await
    };
    let resp = match sent {
        Ok(r) => r,
        Err(e) => {
            if ctx.expire_if_needed(&e) {
                bail!("{RELOGIN}");
            }
            return Err(e.context("Gửi tin nhắn thất bại"));
        }
    };

    // Keep a local echo so `zline read` shows it immediately; the server copy has the same ID
    // and is merged when it arrives. Without an ID we skip it rather than risk a duplicate.
    let mid = msg_id(&resp);
    if let Err(e) = persist_echo(&id, &name, is_group, &text, &mid, None) {
        eprintln!("! Đã gửi nhưng không lưu được bản sao cục bộ: {e:#}");
    }

    if ctx.json {
        let v = json!({ "ok": true, "thread_id": id, "thread": name, "msg_id": mid });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        eprintln!("✓ Đã gửi tới {}", crate::fmt::sanitize(&name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_stdin() -> Result<String> {
        panic!("stdin must not be read when arguments are given")
    }

    #[test]
    fn joins_arguments_with_spaces() {
        let args = vec!["xin".to_string(), "chào".to_string()];
        assert_eq!(compose(&args, no_stdin).unwrap(), "xin chào");
    }

    #[test]
    fn dash_or_empty_reads_stdin_and_trims_trailing_newline() {
        let dash = vec!["-".to_string()];
        assert_eq!(compose(&dash, || Ok("a\nb\n".into())).unwrap(), "a\nb");
        assert_eq!(compose(&[], || Ok("x\r\n".into())).unwrap(), "x");
    }

    #[test]
    fn blank_message_is_rejected() {
        assert!(compose(&["  ".to_string()], no_stdin).is_err());
        assert!(compose(&[], || Ok("\n".into())).is_err());
    }

    #[test]
    fn extracts_msg_id_from_string_or_number() {
        assert_eq!(msg_id(&json!({"data": {"msgId": "123"}})), "123");
        assert_eq!(msg_id(&json!({"data": {"msgId": 456}})), "456");
        assert_eq!(msg_id(&json!({"data": {}})), "");
    }
}
