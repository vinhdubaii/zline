use anyhow::{bail, Result};
use zalo_rust::chat::{self, PollState, Thread};
use zalo_rust::history_sync;
use zalo_rust::session::{Session, SessionStatus};
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

/// Result of a quiet refresh: nothing is printed, so the TUI can use it too.
pub struct Refreshed {
    pub threads: Vec<Thread>,
    pub new_msgs: usize,
    /// Non-fatal problems (for example one endpoint failing).
    pub warnings: Vec<String>,
}

/// Pull friends, groups and recent messages from the server into the local cache and SQLite.
///
/// Returns `Err` only for failures that stop the whole refresh; callers can check
/// `SessionStatus::from_error` to see whether the session itself was refused.
pub async fn refresh_core(s: &Session) -> Result<Refreshed> {
    let mut all = threads::load_local();
    let mut warnings = Vec::new();

    match chat::get_friends(s).await {
        Ok(list) => merge(&mut all, list),
        Err(e) if SessionStatus::from_error(&e) == SessionStatus::Expired => return Err(e),
        Err(e) => warnings.push(format!("Không tải được danh sách bạn bè: {e:#}")),
    }
    match chat::get_groups(s).await {
        Ok(list) => merge(&mut all, list),
        Err(e) if SessionStatus::from_error(&e) == SessionStatus::Expired => return Err(e),
        Err(e) => warnings.push(format!("Không tải được danh sách nhóm: {e:#}")),
    }
    let mut state = PollState::default();
    match chat::poll_new_messages(s, &mut state, &mut all).await {
        Ok(()) => {}
        Err(e) if SessionStatus::from_error(&e) == SessionStatus::Expired => return Err(e),
        Err(e) => warnings.push(format!("Không tải được tin nhắn mới: {e:#}")),
    }

    let new_msgs = store::persist_threads(&mut MessageStore::open(DB_FILE)?, &all)?;
    chat::save_threads_cache(&all);
    Ok(Refreshed {
        threads: all,
        new_msgs,
        warnings,
    })
}

/// [`refresh_core`] with progress on stderr and session-expiry bookkeeping, for the CLI.
pub async fn refresh(ctx: &mut Ctx) -> Result<Vec<Thread>> {
    match refresh_core(&ctx.s).await {
        Ok(r) => {
            for w in &r.warnings {
                eprintln!("! {w}");
            }
            eprintln!("✓ Đã đồng bộ {} hội thoại, {} tin mới.", r.threads.len(), r.new_msgs);
            Ok(r.threads)
        }
        Err(e) => {
            if ctx.expire_if_needed(&e) {
                bail!("{RELOGIN}");
            }
            Err(e)
        }
    }
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
