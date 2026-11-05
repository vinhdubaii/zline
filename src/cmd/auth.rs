use anyhow::{bail, Context, Result};
use serde_json::json;
use zalo_rust::chat;
use zalo_rust::qr;
use zalo_rust::session::{redact_url, Session, SessionError};
use zalo_rust::session::SessionStatus;

use crate::cli::Global;
use crate::cookies::{self, Snapshot};
use crate::ctx::{Ctx, CACHE_FILE, COOKIE_FILE, DB_FILE, SESSION_FILE};

fn remove_if_exists(path: &str) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("Không xóa được {path}")),
    }
}

/// One cheap authenticated call, to learn whether the server accepts this session right now.
async fn probe(ctx: &Ctx) -> std::result::Result<usize, String> {
    chat::get_friends(&ctx.s)
        .await
        .map(|list| list.len())
        .map_err(|e| describe_error(&e))
}

/// Status of a brand-new session rebuilt from what is on disk, optionally with extra cookies or
/// an alternative `zpw_sek` applied. This is what the next `zline` command will start from.
async fn fresh_status(
    proxy: Option<&str>,
    cookies_snap: Option<&Snapshot>,
    sek: Option<&str>,
) -> SessionStatus {
    let Ok(mut t) = Session::new(false, false, false) else {
        return SessionStatus::Unreachable;
    };
    if let Some(p) = proxy {
        let _ = t.set_proxy(Some(p));
    }
    if t.load_from_file(SESSION_FILE).is_err() {
        return SessionStatus::Unreachable;
    }
    if let Some(snap) = cookies_snap {
        cookies::inject(&t.jar, snap);
    }
    if let Some(v) = sek {
        t.set_zpw_sek(v.to_string());
    }
    t.session_status().await
}

/// A login that only works inside the process that created it is useless for a CLI. Rebuild a
/// fresh session from disk and ask the server; if it is refused, try the likely remedies and keep
/// the first one that works.
async fn verify_restore(ctx: &mut Ctx) {
    eprintln!("… Kiểm tra phiên có khôi phục được ở lần chạy sau không");
    let proxy = ctx.proxy.clone();
    match fresh_status(proxy.as_deref(), None, None).await {
        SessionStatus::Valid => {
            eprintln!("✓ Phiên khôi phục được từ session.json.");
            return;
        }
        SessionStatus::Unreachable => {
            eprintln!("! Không có kết luận (mạng lỗi?). Thử `zline sync` để biết phiên có dùng được không.");
            return;
        }
        SessionStatus::Expired => {}
    }

    let snap = cookies::capture(&ctx.s.jar, &cookies::hosts(&ctx.s));
    let values = cookies::distinct_values(&snap, "zpw_sek");
    let saved = ctx.s.zpw_sek.clone();
    let saved_in_jar = values.iter().any(|v| Some(v.as_str()) == saved.as_deref());
    eprintln!(
        "! Phiên vừa lưu bị máy chủ từ chối khi nạp lại. Cookie jar: {} host, {} giá trị zpw_sek khác nhau, khóa đã lưu {}.",
        snap.len(),
        values.len(),
        if saved_in_jar { "có trong jar" } else { "KHÔNG có trong jar" }
    );

    // Remedy 1: persist the whole cookie jar and restore it on every start.
    if fresh_status(proxy.as_deref(), Some(&snap), None).await == SessionStatus::Valid {
        match cookies::save(&snap, std::path::Path::new(COOKIE_FILE)) {
            Ok(()) => eprintln!("✓ Đã khắc phục: lưu thêm cookie vào {COOKIE_FILE} (phiên giờ khôi phục được)."),
            Err(e) => eprintln!("✗ Cookie đủ để khôi phục nhưng không ghi được {COOKIE_FILE}: {e:#}"),
        }
        return;
    }

    // Remedy 2: the saved key is not the one the server set; use another value from the jar.
    for v in values.iter().filter(|v| Some(v.as_str()) != saved.as_deref()) {
        if fresh_status(proxy.as_deref(), None, Some(v)).await == SessionStatus::Valid {
            ctx.s.set_zpw_sek(v.clone());
            match ctx.s.save_to_file(SESSION_FILE) {
                Ok(()) => eprintln!("✓ Đã khắc phục: dùng khóa phiên lấy từ cookie của máy chủ."),
                Err(e) => eprintln!("✗ Tìm được khóa đúng nhưng không lưu được session.json: {e:#}"),
            }
            return;
        }
    }

    eprintln!("✗ Phiên dùng được trong lần chạy này nhưng không khôi phục được ở lần sau.");
    eprintln!("  Hãy chạy `zline doctor` và gửi kết quả cùng dòng \"Cookie jar\" ở trên.");
}

pub async fn login(g: &Global, force: bool) -> Result<()> {
    let mut ctx = Ctx::local(g)?;
    if !force && ctx.logged_in() {
        if let Some(li) = ctx.s.login.as_ref() {
            eprintln!(
                "Đã đăng nhập: {} (UIN {}). Dùng --force để đăng nhập lại.",
                li.display_name, li.uin
            );
        }
        return Ok(());
    }
    // Clear the in-memory "server refused" marker so the fresh login is saved without it. The old
    // credentials on disk are only replaced once the QR login succeeds.
    ctx.s.clear_expired();
    eprintln!("… Đang tạo mã QR");
    qr::login_via_qr(&mut ctx.s, "vi")
        .await
        .context("Đăng nhập bằng QR thất bại")?;
    ctx.s.save_to_file(SESSION_FILE)?;
    // The QR flow does not return the profile; fetch the real name and user ID.
    ctx.refresh_profile().await;
    if let Some(li) = ctx.s.login.as_ref() {
        let name = if li.display_name.is_empty() { "(chưa lấy được tên)" } else { li.display_name.as_str() };
        eprintln!("✓ Xin chào {name} (UIN {}).", li.uin);
    }
    // Check the session in this same process, so a rejection can be told apart from one that only
    // happens after `session.json` is reloaded.
    match probe(&ctx).await {
        Ok(n) => {
            eprintln!("✓ Máy chủ chấp nhận phiên ({n} liên hệ).");
            verify_restore(&mut ctx).await;
            eprintln!("Chạy `zline sync` để tải hội thoại.");
        }
        Err(msg) => {
            eprintln!("✗ Máy chủ từ chối phiên ngay sau khi đăng nhập: {msg}");
            eprintln!("  Mã 600 ở bước này nghĩa là Zalo không nhận khóa phiên vừa cấp — không phải do bạn quét sai.");
            eprintln!("  Chạy `zline doctor` và gửi kết quả (đã ẩn thông tin nhạy cảm) để tìm nguyên nhân.");
        }
    }
    Ok(())
}

pub fn logout(g: &Global, purge: bool) -> Result<()> {
    let ctx = Ctx::local(g)?;
    let mut removed = Vec::new();
    for f in [SESSION_FILE, CACHE_FILE, COOKIE_FILE] {
        if remove_if_exists(f)? {
            removed.push(f);
        }
    }
    if purge {
        for f in [DB_FILE, "zalo_history.db-wal", "zalo_history.db-shm"] {
            if remove_if_exists(f)? {
                removed.push(f);
            }
        }
    }
    if removed.is_empty() {
        eprintln!("Không có gì để xóa trong {}", ctx.dir.display());
    } else {
        eprintln!("✓ Đã xóa: {}", removed.join(", "));
    }
    Ok(())
}

pub async fn whoami(g: &Global, offline: bool) -> Result<()> {
    let ctx = Ctx::local(g)?;
    let Some(li) = ctx.s.login.as_ref() else {
        bail!("Chưa đăng nhập. Chạy `zline login`.");
    };
    let status = if ctx.s.is_expired_marked() {
        "expired"
    } else if offline {
        "unchecked"
    } else {
        match ctx.s.session_status().await {
            SessionStatus::Valid => "valid",
            SessionStatus::Expired => "expired",
            SessionStatus::Unreachable => "unreachable",
        }
    };
    if ctx.json {
        let v = json!({
            "name": li.display_name,
            "uin": li.uin,
            "status": status,
            "data_dir": ctx.dir.display().to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        let label = match status {
            "valid" => "còn hiệu lực",
            "expired" => "đã hết hạn — chạy `zline login`",
            "unreachable" => "không kết nối được máy chủ",
            _ => "chưa kiểm tra với máy chủ",
        };
        println!("{} (UIN {})", li.display_name, li.uin);
        println!("Phiên: {label}");
        println!("Dữ liệu: {}", ctx.dir.display());
    }
    Ok(())
}

/// Remove query strings from any URL inside an error message: they can carry signed request
/// parameters that must not end up in a pasted bug report.
pub fn scrub(msg: &str) -> String {
    msg.split(' ')
        .map(|tok| match (tok.contains("http"), tok.find('?')) {
            (true, Some(i)) => {
                let tail = if tok.ends_with(')') { ")" } else { "" };
                format!("{}?…{tail}", &tok[..i])
            }
            _ => tok.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn describe_error(e: &anyhow::Error) -> String {
    match e.downcast_ref::<SessionError>() {
        Some(se) => {
            let at = if se.url.is_empty() {
                String::new()
            } else {
                format!(" (từ {})", scrub(&redact_url(&se.url)))
            };
            format!("máy chủ trả mã {}: {}{at}", se.code, scrub(&se.message))
        }
        None => scrub(&format!("{e:#}")),
    }
}

/// Step-by-step login diagnostics. Prints no credentials, only presence/absence and server codes.
pub async fn doctor(g: &Global) -> Result<()> {
    let ctx = Ctx::local(g)?;
    let mark = |ok: bool| if ok { "✓" } else { "✗" };
    println!("Thư mục dữ liệu: {}", ctx.dir.display());
    println!("{} session.json", mark(std::path::Path::new(SESSION_FILE).exists()));
    println!("{} zalo_history.db", mark(std::path::Path::new(DB_FILE).exists()));
    if let Some(snap) = cookies::load(std::path::Path::new(COOKIE_FILE)) {
        println!("✓ {COOKIE_FILE}: {} host (cookie bổ sung để khôi phục phiên)", snap.len());
    }

    let Some(li) = ctx.s.login.as_ref() else {
        println!("✗ Chưa nạp được phiên (chưa đăng nhập hoặc session.json hỏng). Chạy `zline login`.");
        return Ok(());
    };
    println!(
        "✓ Đã nạp phiên: {} (UIN {})",
        if li.display_name.is_empty() { "(chưa có tên)" } else { li.display_name.as_str() },
        li.uin
    );
    println!("{} có khóa phiên zpw_sek", mark(!li.zpw_sek.is_empty()));
    {
        let sek = &li.zpw_sek;
        let class = if sek.chars().all(|c| c.is_ascii_hexdigit()) {
            "hex"
        } else if sek.chars().all(|c| c.is_ascii_alphanumeric() || "+/=-_.".contains(c)) {
            "chữ-số/base64"
        } else {
            "có ký tự lạ"
        };
        println!("  (zpw_sek dài {} ký tự, dạng {class}; không in nội dung)", sek.len());
        println!("  endpoint hồ sơ: {}", ctx.s.domains.profile);
        println!("  endpoint bạn bè: {}", ctx.s.domains.friend);
        println!("  endpoint xác thực: {}", ctx.s.domains.auth);
    }
    println!("{} có khóa zpw_enk", mark(!li.zpw_enk.is_empty()));
    println!(
        "{} máy chủ chưa từng từ chối phiên này (dấu hết hạn cục bộ: {})",
        mark(!ctx.s.is_expired_marked()),
        if ctx.s.is_expired_marked() { "có" } else { "không" }
    );

    let status = ctx.s.session_status().await;
    println!(
        "{} kiểm tra phiên với máy chủ: {}",
        mark(status == SessionStatus::Valid),
        match status {
            SessionStatus::Valid => "hợp lệ",
            SessionStatus::Expired => "máy chủ từ chối (mã 600 hoặc -102)",
            SessionStatus::Unreachable => "không có kết luận (mạng lỗi hoặc mã lỗi khác)",
        }
    );
    match chat::get_friends(&ctx.s).await {
        Ok(list) => println!("✓ tải danh bạ: {} liên hệ", list.len()),
        Err(e) => println!("✗ tải danh bạ: {}", describe_error(&e)),
    }
    println!("\nNếu có dòng ✗, hãy gửi toàn bộ kết quả này (không chứa mật khẩu hay khóa).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_removes_query_strings_from_urls() {
        let s = scrub("error sending request for url (https://x.zalo.me/api?zpw_ver=1&signkey=SECRET)");
        assert!(!s.contains("SECRET") && !s.contains("zpw_ver"));
        assert!(s.contains("https://x.zalo.me/api?…)"));
        assert_eq!(scrub("không có url? gì"), "không có url? gì");
    }
}
