use anyhow::Result;
use serde_json::{json, Value};
use zalo_rust::store::MessageStore;

use crate::cli::Global;
use crate::ctx::{Ctx, DB_FILE};
use crate::fmt::{day, hm, sanitize};
use crate::threads;

/// Read stored messages. This never touches the network: `zline sync` fills the local store.
pub fn run(g: &Global, thread: &str, count: usize) -> Result<()> {
    let ctx = Ctx::local(g)?;
    let mut all = threads::load_local();
    threads::ensure_cloud(&mut all, &ctx.s);
    let cloud = ctx.cloud_id().map(str::to_owned);
    let t = threads::expect_one(threads::resolve(&all, thread, cloud.as_deref()), thread)?;

    let msgs = MessageStore::open(DB_FILE)?.messages_for(&t.id, count.max(1))?;

    if ctx.json {
        let arr: Vec<Value> = msgs
            .iter()
            .map(|m| {
                json!({
                    "id": m.msg_id,
                    "ts": m.ts,
                    "sender_id": m.sender_id,
                    "sender": m.sender_name,
                    "text": m.text,
                    "type": m.msg_type,
                    "is_outgoing": m.is_outgoing,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
        return Ok(());
    }

    println!(
        "── {} ({}) ──",
        sanitize(&t.name),
        if t.is_group { "nhóm" } else { "1-1" }
    );
    if msgs.is_empty() {
        eprintln!("Chưa có tin nhắn nào được lưu cục bộ. Chạy `zline sync` trước.");
        return Ok(());
    }
    let mut last_day = String::new();
    for m in &msgs {
        let d = day(m.ts);
        if d != last_day {
            println!("\n── {d} ──");
            last_day = d;
        }
        let who = if m.is_outgoing {
            "Bạn".to_string()
        } else if m.sender_name.is_empty() {
            m.sender_id.clone()
        } else {
            m.sender_name.clone()
        };
        let body = if m.text.trim().is_empty() {
            format!("[{}]", if m.msg_type.is_empty() { "tin nhắn" } else { m.msg_type.as_str() })
        } else {
            sanitize(&m.text)
        };
        let mut lines = body.lines();
        println!("{}  {}: {}", hm(m.ts), sanitize(&who), lines.next().unwrap_or(""));
        for l in lines {
            println!("       {l}");
        }
    }
    Ok(())
}
