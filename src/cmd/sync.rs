use anyhow::{bail, Result};
use zalo_rust::chat::{self, PollState, Thread};
use zalo_rust::history_sync;
use zalo_rust::store::{self, MessageStore};

use crate::cli::Global;
use crate::ctx::{Ctx, DB_FILE, RELOGIN};
use crate::threads;

/// Merge freshly fetched conversations into `all` without losing local history.
fn merge(all: &mut Vec<Thread>, incoming: Vec<Thread>) {
    for t in incoming {
        if let Some(existing) = all.iter_mut().find(|x| x.id == t.id) {
            if !t.name.is_empty() {
                existing.name = t.name;
            }
            existing.is_group = existing.is_group || t.is_group;
        } else {
            all.push(t);
        }
    }
}

/// Pull friends, groups and recent messages from the server into the local cache and SQLite.
pub async fn refresh(ctx: &mut Ctx) -> Result<Vec<Thread>> {
    let mut all = threads::load_local();

    let friends = chat::get_friends(&ctx.s).await;
    match friends {
        Ok(list) => merge(&mut all, list),
        Err(e) => {
            if ctx.expire_if_needed(&e) {
                bail!("{RELOGIN}");
            }
            eprintln!("! Không tải được danh sách bạn bè: {e:#}");
        }
    }

    let groups = chat::get_groups(&ctx.s).await;
    match groups {
        Ok(list) => merge(&mut all, list),
        Err(e) => {
            if ctx.expire_if_needed(&e) {
                bail!("{RELOGIN}");
            }
            eprintln!("! Không tải được danh sách nhóm: {e:#}");
        }
    }

    let mut state = PollState::default();
    let polled = chat::poll_new_messages(&ctx.s, &mut state, &mut all).await;
    if let Err(e) = polled {
        if ctx.expire_if_needed(&e) {
            bail!("{RELOGIN}");
        }
        eprintln!("! Không tải được tin nhắn mới: {e:#}");
    }

    let new = store::persist_threads(&mut MessageStore::open(DB_FILE)?, &all)?;
    chat::save_threads_cache(&all);
    eprintln!("✓ Đã đồng bộ {} hội thoại, {new} tin mới.", all.len());
    Ok(all)
}

pub async fn run(g: &Global, thread: Option<String>) -> Result<()> {
    let mut ctx = Ctx::online(g).await?;
    let Some(query) = thread else {
        refresh(&mut ctx).await?;
        return Ok(());
    };

    let mut all = threads::load_local();
    threads::ensure_cloud(&mut all, &ctx.s);
    let cloud = ctx.cloud_id().map(str::to_owned);
    let found = threads::expect_one(threads::resolve(&all, &query, cloud.as_deref()), &query)?;
    let is_cloud = cloud.as_deref() == Some(found.id.as_str());
    if !(found.is_group || is_cloud) {
        bail!("Chỉ kéo được lịch sử cũ của nhóm và Cloud; hội thoại 1-1 nhận tin mới qua `zline sync`.");
    }
    let mut t = Thread {
        id: found.id.clone(),
        name: found.name.clone(),
        is_group: found.is_group,
        ..Default::default()
    };
    eprintln!("… Đang kéo lịch sử của {} (tối đa 1000 tin mỗi lần chạy)", t.name);
    let res = history_sync::sync_thread(&ctx.s, &mut t).await;
    match res {
        Ok(added) => eprintln!("✓ Thêm {added} tin vào lịch sử."),
        Err(e) => {
            if ctx.expire_if_needed(&e) {
                bail!("{RELOGIN}");
            }
            return Err(e.context("Kéo lịch sử thất bại"));
        }
    }
    Ok(())
}
