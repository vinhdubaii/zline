use anyhow::{bail, Context, Result};
use serde_json::json;
use zalo_rust::qr;
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
    if let Some(li) = ctx.s.login.as_ref() {
        eprintln!("✓ Xin chào {} (UIN {}). Chạy `zline sync` để tải hội thoại.", li.display_name, li.uin);
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
