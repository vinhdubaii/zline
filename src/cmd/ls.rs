use anyhow::Result;
use serde_json::{json, Value};
use zalo_rust::chat::Thread;

use crate::cli::Global;
use crate::cmd::sync;
use crate::ctx::Ctx;
use crate::fmt::{clip, pad, short_time};
use crate::threads;

/// Preview line: the stored preview, else the text of the newest message we hold.
fn preview(t: &Thread) -> String {
    if !t.preview.is_empty() {
        return t.preview.clone();
    }
    t.messages
        .iter()
        .max_by_key(|m| zalo_rust::clock::normalize_ts_ms(m.ts))
        .map(|m| m.text.clone())
        .unwrap_or_default()
}

pub async fn run(g: &Global, groups: bool, friends: bool, refresh: bool, limit: usize) -> Result<()> {
    let mut ctx = if refresh {
        Ctx::online(g).await?
    } else {
        Ctx::local(g)?
    };
    let mut list = if refresh {
        sync::refresh(&mut ctx).await?
    } else {
        threads::load_local()
    };
    let nothing_local = list.is_empty();
    threads::ensure_cloud(&mut list, &ctx.s);
    threads::sort_recent(&mut list);
    list.retain(|t| (!groups || t.is_group) && (!friends || !t.is_group));
    if limit > 0 {
        list.truncate(limit);
    }

    if ctx.json {
        let arr: Vec<Value> = list
            .iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "name": t.name,
                    "is_group": t.is_group,
                    "last_ts": threads::last_ts(t),
                    "preview": preview(t),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
    } else {
        println!(
            "{} {} {} {} {}",
            pad("LOẠI", 5),
            pad("GẦN NHẤT", 11),
            pad("TÊN", 28),
            pad("TIN CUỐI", 40),
            "ID"
        );
        for t in &list {
            println!(
                "{} {} {} {} {}",
                pad(if t.is_group { "nhóm" } else { "bạn" }, 5),
                pad(&short_time(threads::last_ts(t)), 11),
                pad(&clip(&t.name, 28), 28),
                pad(&clip(&preview(t), 40), 40),
                t.id
            );
        }
    }
    if nothing_local && !refresh {
        eprintln!("Chưa có hội thoại lưu cục bộ. Chạy `zline sync` (hoặc `zline ls --refresh`).");
    }
    Ok(())
}
