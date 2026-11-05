//! Keeping the cookie jar next to `session.json`.
//!
//! `session.json` stores a single value, `zpw_sek`. The ctdozlite app itself runs in one process
//! and keeps using the live cookie jar that the server filled during login. Z-Line runs one process
//! per command, so everything a later command needs must survive on disk. After a QR login Z-Line
//! checks whether a *fresh* session built from `session.json` is accepted, and if not, falls back
//! to persisting the jar (this file).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use reqwest::cookie::{CookieStore, Jar};
use zalo_rust::data_dir::write_private_atomic;
use zalo_rust::session::Session;

pub const COOKIE_FILE: &str = "cookies.json";

/// Request URL -> the `(name, value)` cookie pairs the jar would send to it.
pub type Snapshot = BTreeMap<String, Vec<(String, String)>>;

/// Split a `Cookie` request-header value into pairs.
pub fn parse_pairs(header: &str) -> Vec<(String, String)> {
    header
        .split(';')
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            let name = name.trim();
            (!name.is_empty()).then(|| (name.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// Hosts worth asking the jar about: the ones ctdozlite attaches `zpw_sek` to, plus whatever the
/// current service map points at.
pub fn hosts(s: &Session) -> Vec<String> {
    let d = &s.domains;
    let mut list: Vec<String> = [
        "https://chat.zalo.me",
        "https://wpa.zaloapp.com",
        "https://wpa.chat.zalo.me",
        "https://tt-sticker-wpa.chat.zalo.me",
        "https://sticker-wpa.chat.zalo.me",
    ]
    .iter()
    .map(|h| h.to_string())
    .collect();
    for h in [
        &d.auth,
        &d.chat,
        &d.conversation,
        &d.lp_chat,
        &d.lp_control,
        &d.convers_api,
        &d.profile,
        &d.friend,
        &d.group,
        &d.cloud_msg,
        &d.voice_call,
        &d.sticker,
    ] {
        if !h.is_empty() && !list.contains(h) {
            list.push(h.clone());
        }
    }
    list
}

pub fn capture(jar: &Jar, hosts: &[String]) -> Snapshot {
    let mut out = Snapshot::new();
    for h in hosts {
        let Ok(url) = reqwest::Url::parse(h) else { continue };
        let Some(header) = jar.cookies(&url) else { continue };
        let Ok(text) = header.to_str() else { continue };
        let pairs = parse_pairs(text);
        if !pairs.is_empty() {
            out.insert(h.clone(), pairs);
        }
    }
    out
}

pub fn inject(jar: &Jar, snap: &Snapshot) {
    for (host, pairs) in snap {
        let Ok(url) = reqwest::Url::parse(host) else { continue };
        for (name, value) in pairs {
            jar.add_cookie_str(&format!("{name}={value}; Path=/"), &url);
        }
    }
}

/// Every distinct value of cookie `name`, in first-seen order.
pub fn distinct_values(snap: &Snapshot, name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for pairs in snap.values() {
        for (n, v) in pairs {
            if n == name && !v.is_empty() && !out.contains(v) {
                out.push(v.clone());
            }
        }
    }
    out
}

pub fn save(snap: &Snapshot, path: &Path) -> Result<()> {
    write_private_atomic(path, serde_json::to_string_pretty(snap)?.as_bytes())
}

pub fn load(path: &Path) -> Option<Snapshot> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cookie_headers() {
        assert_eq!(
            parse_pairs("a=1; b=x=y ; ;=bad; c="),
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "x=y".to_string()),
                ("c".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn capture_and_inject_round_trip_through_a_jar() {
        let host = "https://tt-profile-wpa.chat.zalo.me".to_string();
        let url = reqwest::Url::parse(&host).unwrap();
        let src = Jar::default();
        src.add_cookie_str("zpw_sek=AAA; Path=/", &url);
        src.add_cookie_str("zpsid=BBB; Path=/", &url);
        let snap = capture(&src, std::slice::from_ref(&host));
        assert_eq!(snap[&host].len(), 2);

        let dst = Jar::default();
        inject(&dst, &snap);
        assert_eq!(capture(&dst, std::slice::from_ref(&host)), snap);
    }

    #[test]
    fn distinct_values_keeps_order_and_skips_duplicates() {
        let mut snap = Snapshot::new();
        snap.insert("h1".into(), vec![("zpw_sek".into(), "A".into()), ("x".into(), "1".into())]);
        snap.insert("h2".into(), vec![("zpw_sek".into(), "A".into()), ("zpw_sek".into(), "B".into())]);
        assert_eq!(distinct_values(&snap, "zpw_sek"), vec!["A", "B"]);
        assert!(distinct_values(&snap, "none").is_empty());
    }

    #[test]
    fn missing_or_corrupt_file_loads_as_none() {
        let dir = std::env::temp_dir().join(format!("zl-cookies-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(COOKIE_FILE);
        assert!(load(&p).is_none());
        std::fs::write(&p, "{broken").unwrap();
        assert!(load(&p).is_none());
        let mut snap = Snapshot::new();
        snap.insert("h".into(), vec![("a".into(), "1".into())]);
        save(&snap, &p).unwrap();
        assert_eq!(load(&p), Some(snap));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
