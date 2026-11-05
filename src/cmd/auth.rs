use anyhow::{bail, Context, Result};
use serde_json::json;
use zalo_rust::chat;
use zalo_rust::qr;
use zalo_rust::session::{redact_url, SessionError};
use zalo_rust::session::SessionStatus;

use crate::cli::Global;
use crate::ctx::{Ctx, CACHE_FILE, DB_FILE, SESSION_FILE};

fn remove_if_exists(path: &str) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("Không xóa được {path}")),
    }
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
        eprintln!("✓ Xin chào {name} (UIN {}). Chạy `zline sync` để tải hội thoại.", li.uin);
    }
    Ok(())
}

pub fn logout(g: &Global, purge: bool) -> Result<()> {
    let ctx = Ctx::local(g)?;
    let mut removed = Vec::new();
    for f in [SESSION_FILE, CACHE_FILE] {
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
