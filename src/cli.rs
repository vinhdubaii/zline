//! Command-line surface of Z-Line.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "zline",
    version,
    about = "Z-Line — Zalo cho terminal",
    long_about = "Z-Line — Zalo cho terminal.\n\nDùng lõi giao thức của ctdozlite. Client không chính thức: nên thử bằng tài khoản phụ."
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,
    /// Không có lệnh con thì mở giao diện TUI.
    #[command(subcommand)]
    pub cmd: Option<Command>,
}

/// Options accepted by every subcommand.
#[derive(Args, Clone, Debug)]
pub struct Global {
    /// Thư mục dữ liệu (session, cache, lịch sử SQLite). Mặc định: <data home>/z-line/profiles/<profile>
    #[arg(long, global = true, value_name = "DIR", env = "ZLINE_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// Tên hồ sơ tài khoản (mỗi hồ sơ có session và lịch sử riêng)
    #[arg(long, global = true, default_value = "default", env = "ZLINE_PROFILE")]
    pub profile: String,

    /// Proxy HTTP/HTTPS/SOCKS5 cho phiên này
    #[arg(long, global = true, value_name = "URL")]
    pub proxy: Option<String>,

    /// In kết quả dạng JSON (dễ dùng trong script)
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Mở giao diện TUI (mặc định khi không có lệnh con)
    Tui,
    /// Đăng nhập bằng mã QR (quét bằng Zalo trên điện thoại)
    Login {
        /// Đăng nhập lại dù phiên hiện tại vẫn còn hiệu lực
        #[arg(long)]
        force: bool,
    },
    /// Xóa phiên đăng nhập và cache hội thoại
    Logout {
        /// Xóa luôn lịch sử tin nhắn (zalo_history.db)
        #[arg(long)]
        purge: bool,
    },
    /// Hiện tài khoản đang đăng nhập và tình trạng phiên
    Whoami {
        /// Không hỏi máy chủ, chỉ đọc phiên đã lưu
        #[arg(long)]
        offline: bool,
    },
    /// Liệt kê hội thoại (mới nhất trước)
    Ls {
        /// Chỉ nhóm
        #[arg(long, conflicts_with = "friends")]
        groups: bool,
        /// Chỉ hội thoại 1-1
        #[arg(long)]
        friends: bool,
        /// Tải lại danh sách và tin mới từ máy chủ trước khi in
        #[arg(long)]
        refresh: bool,
        /// Số dòng tối đa (0 = không giới hạn)
        #[arg(short = 'n', long, default_value_t = 0)]
        limit: usize,
    },
    /// Đọc tin nhắn đã lưu của một hội thoại
    Read {
        /// ID hoặc tên (khớp một phần, không phân biệt dấu); `cloud` = Cloud của tôi
        thread: String,
        /// Số tin gần nhất
        #[arg(short = 'n', long, default_value_t = 30)]
        count: usize,
    },
    /// Gửi tin nhắn văn bản (không có nội dung hoặc `-` thì đọc từ stdin)
    Send {
        /// ID hoặc tên hội thoại; `cloud` = Cloud của tôi
        thread: String,
        /// Nội dung tin nhắn
        text: Vec<String>,
    },
    /// Đồng bộ danh sách hội thoại và tin mới; với <thread> thì kéo lịch sử cũ (nhóm và Cloud)
    Sync {
        thread: Option<String>,
    },
}
