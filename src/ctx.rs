//! Per-run context: data directory, session and output mode.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use zalo_rust::data_dir;
use zalo_rust::session::{Session, SessionStatus};

use crate::cli::Global;
use crate::cookies;
use crate::paths;

pub const SESSION_FILE: &str = "session.json";
pub const CACHE_FILE: &str = "threads_cache.json";
pub use crate::cookies::COOKIE_FILE;
pub const DB_FILE: &str = "zalo_history.db";
pub const RELOGIN: &str = "Phiên đăng nhập đã hết hạn. Chạy `zline login` để đăng nhập lại. (Nếu vừa đăng nhập xong mà lỗi này lặp lại ngay, chạy `zline doctor` và gửi kết quả.)";

pub struct Ctx {
    pub s: Session,
    pub dir: PathBuf,
    pub json: bool,
    pub proxy: Option<String>,
}

impl Ctx {
    /// Select the data directory and load the saved session *without touching the network*.
    ///
    /// `zalo_rust` reads and writes `session.json`, `zalo_history.db` and `threads_cache.json`
    /// relative to the working directory, so `data_dir::initialize` also changes into it. It must
    /// therefore run before any other file access.
    pub fn local(g: &Global) -> Result<Self> {
        let dir = paths::resolve(g.data_dir.as_deref(), &g.profile)?;
        let args = vec!["--data-dir".to_string(), dir.to_string_lossy().into_owned()];
        let dir = data_dir::initialize(&args)?;

        let mut s = Session::new(false, false, false).context("Không khởi tạo được phiên HTTP")?;
        if let Some(p) = g.proxy.as_deref() {
            s.set_proxy(Some(p))?;
        }
        if Path::new(SESSION_FILE).exists() {
            // A corrupt file is treated as "not logged in"; `login` will replace it.
            let _ = s.load_from_file(SESSION_FILE);
        }
        if s.login.is_some() {
            // Only present when a fresh session built from `session.json` alone was not accepted
            // after login (see `cmd::auth::verify_restore`).
            if let Some(snap) = cookies::load(Path::new(COOKIE_FILE)) {
                cookies::inject(&s.jar, &snap);
            }
        }
        Ok(Self {
            s,
            dir,
            json: g.json,
            proxy: g.proxy.clone(),
        })
    }

    /// Like [`Ctx::local`], but requires a live session and checks it with the server.
    pub async fn online(g: &Global) -> Result<Self> {
        let mut ctx = Self::local(g)?;
        if ctx.s.login.is_none() {
            bail!("Chưa đăng nhập. Chạy `zline login` trước.");
        }
        if ctx.s.is_expired_marked() {
            bail!("{RELOGIN}");
        }
        match ctx.s.session_status().await {
            SessionStatus::Valid => {
                let unnamed = ctx
                    .s
                    .login
                    .as_ref()
                    .is_some_and(|l| l.display_name.is_empty());
                if unnamed {
                    ctx.refresh_profile().await;
                }
            }
            SessionStatus::Expired => {
                ctx.expire();
                bail!("{RELOGIN}");
            }
            SessionStatus::Unreachable => {
                eprintln!("⚠️ Không kết nối được máy chủ Zalo; các thao tác mạng có thể thất bại.");
            }
        }
        Ok(ctx)
    }

    /// Ask the server who we are and store the real user ID and display name. A fresh QR login
    /// only knows the account by a login-scoped ID with no name, so this mirrors what ctdozlite
    /// does right after start-up. Best effort: returns `false` when the server gave no profile.
    pub async fn refresh_profile(&mut self) -> bool {
        if self.s.sync_profile_me().await.is_some() {
            let _ = self.s.save_to_file(SESSION_FILE);
            true
        } else {
            false
        }
    }

    pub fn logged_in(&self) -> bool {
        self.s.login.is_some() && !self.s.is_expired_marked()
    }

    /// ID of the "Cloud của tôi" (send-to-me) conversation, when logged in.
    pub fn cloud_id(&self) -> Option<&str> {
        self.s
            .login
            .as_ref()
            .map(|l| l.send2me_id.as_str())
            .filter(|id| !id.is_empty())
    }

    /// Record that the server refused the credentials. Only the marker is written; the old
    /// credentials stay on disk until a new login replaces them.
    fn expire(&mut self) {
        self.s.mark_expired();
        let _ = self.s.save_to_file(SESSION_FILE);
    }

    /// If `err` is a server verdict that the session is gone, mark it and return `true`.
    /// Transport errors and unrelated error codes never expire the session.
    pub fn expire_if_needed(&mut self, err: &anyhow::Error) -> bool {
        if SessionStatus::from_error(err) == SessionStatus::Expired {
            self.expire();
            true
        } else {
            false
        }
    }
}
