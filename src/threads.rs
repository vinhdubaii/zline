//! Loading, ordering and looking up conversations.

use zalo_rust::chat::{self, Thread};
use zalo_rust::clock;
use zalo_rust::session::Session;
use zalo_rust::store::{self, MessageStore};

use crate::ctx::DB_FILE;

pub const CLOUD_NAME: &str = "Cloud của tôi (Lưu trữ)";

/// Conversations known locally: `threads_cache.json` merged with `zalo_history.db`.
pub fn load_local() -> Vec<Thread> {
    let mut threads = chat::load_threads_cache();
    store::load_history_into_threads(DB_FILE, &mut threads, MessageStore::PRELOAD_MSGS);
    threads
}

/// Newest activity of a thread in epoch milliseconds (0 when unknown).
pub fn last_ts(t: &Thread) -> i64 {
    t.messages
        .iter()
        .map(|m| clock::normalize_ts_ms(m.ts))
        .max()
        .unwrap_or(0)
        .max(clock::normalize_ts_ms(t.presentation.last_message_ts))
}

pub fn sort_recent(threads: &mut [Thread]) {
    threads.sort_by_key(|t| std::cmp::Reverse(last_ts(t)));
}

/// Make sure the "Cloud của tôi" conversation is listed once a session is known.
pub fn ensure_cloud(threads: &mut Vec<Thread>, s: &Session) {
    let Some(id) = s
        .login
        .as_ref()
        .map(|l| l.send2me_id.clone())
        .filter(|id| !id.is_empty())
    else {
        return;
    };
    if let Some(t) = threads.iter_mut().find(|t| t.id == id) {
        if t.name.is_empty() {
            t.name = CLOUD_NAME.into();
        }
    } else {
        threads.push(Thread {
            id,
            name: CLOUD_NAME.into(),
            ..Default::default()
        });
    }
}

/// Lowercase ASCII form used for accent-insensitive matching ("Nguyễn" matches "nguyen").
pub fn fold(s: &str) -> String {
    chat::strip_vn_for_sticker_kw(s)
}

pub enum Match<'a> {
    One(&'a Thread),
    Many(Vec<&'a Thread>),
    NotFound,
}

fn pick(mut v: Vec<&Thread>) -> Option<Match<'_>> {
    match v.len() {
        0 => None,
        1 => Some(Match::One(v.remove(0))),
        _ => Some(Match::Many(v)),
    }
}

/// Find a conversation by exact ID, then exact name, then partial name (accent-insensitive).
/// `cloud` and `me` are aliases for the Cloud conversation when its ID is known.
/// An ambiguous query returns every candidate instead of guessing.
pub fn resolve<'a>(threads: &'a [Thread], query: &str, cloud_id: Option<&str>) -> Match<'a> {
    let q = query.trim();
    if q.is_empty() {
        return Match::NotFound;
    }
    if let Some(t) = threads.iter().find(|t| t.id == q) {
        return Match::One(t);
    }
    if matches!(q.to_lowercase().as_str(), "cloud" | "me") {
        if let Some(t) = cloud_id.and_then(|id| threads.iter().find(|t| t.id == id)) {
            return Match::One(t);
        }
    }
    let fq = fold(q);
    if fq.is_empty() {
        return Match::NotFound;
    }
    if let Some(m) = pick(threads.iter().filter(|t| fold(&t.name) == fq).collect()) {
        return m;
    }
    pick(threads.iter().filter(|t| fold(&t.name).contains(&fq)).collect()).unwrap_or(Match::NotFound)
}

/// Turn a [`Match`] into one conversation or a user-facing error that lists candidates.
pub fn expect_one<'a>(m: Match<'a>, query: &str) -> anyhow::Result<&'a Thread> {
    match m {
        Match::One(t) => Ok(t),
        Match::NotFound => anyhow::bail!(
            "Không tìm thấy hội thoại {query:?}. Dùng `zline ls` để xem danh sách (hoặc `zline sync` nếu chưa có)."
        ),
        Match::Many(list) => {
            let mut msg = format!("{query:?} khớp nhiều hội thoại, hãy dùng ID hoặc tên đầy đủ hơn:");
            for t in list.iter().take(10) {
                let kind = if t.is_group { "nhóm" } else { "bạn " };
                msg.push_str(&format!("\n  {kind}  {}  ({})", t.name, t.id));
            }
            if list.len() > 10 {
                msg.push_str(&format!("\n  … và {} kết quả nữa", list.len() - 10));
            }
            anyhow::bail!(msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn th(id: &str, name: &str, is_group: bool) -> Thread {
        Thread {
            id: id.into(),
            name: name.into(),
            is_group,
            ..Default::default()
        }
    }

    fn sample() -> Vec<Thread> {
        vec![
            th("1", "Nguyễn Văn An", false),
            th("2", "Nguyễn Văn Bình", false),
            th("3", "Lớp 12A1", true),
            th("4", "Cloud của tôi (Lưu trữ)", false),
        ]
    }

    #[test]
    fn exact_id_wins() {
        let t = sample();
        assert!(matches!(resolve(&t, "3", None), Match::One(x) if x.name == "Lớp 12A1"));
    }

    #[test]
    fn accent_insensitive_exact_name() {
        let t = sample();
        assert!(matches!(resolve(&t, "nguyen van an", None), Match::One(x) if x.id == "1"));
    }

    #[test]
    fn partial_name_unique() {
        let t = sample();
        assert!(matches!(resolve(&t, "lop 12", None), Match::One(x) if x.id == "3"));
    }

    #[test]
    fn partial_name_ambiguous_lists_all() {
        let t = sample();
        match resolve(&t, "nguyen", None) {
            Match::Many(v) => assert_eq!(v.len(), 2),
            _ => panic!("expected ambiguity"),
        }
    }

    #[test]
    fn cloud_alias_needs_known_id() {
        let t = sample();
        assert!(matches!(resolve(&t, "cloud", Some("4")), Match::One(x) if x.id == "4"));
        assert!(matches!(resolve(&t, "me", Some("4")), Match::One(x) if x.id == "4"));
        assert!(matches!(resolve(&t, "me", None), Match::NotFound));
    }

    #[test]
    fn empty_and_unknown_queries() {
        let t = sample();
        assert!(matches!(resolve(&t, "  ", None), Match::NotFound));
        assert!(matches!(resolve(&t, "zzz", None), Match::NotFound));
    }

    #[test]
    fn ambiguity_error_names_candidates() {
        let t = sample();
        let err = expect_one(resolve(&t, "nguyen", None), "nguyen").unwrap_err().to_string();
        assert!(err.contains("Nguyễn Văn An") && err.contains("Nguyễn Văn Bình"));
    }
}
