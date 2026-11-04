# Z-Line

Zalo client cho terminal, viết bằng Rust. Dùng lõi giao thức của [ctdozlite](https://github.com/ctdoteam1110/ctdozlite) (crate `zalo_rust`, ghim theo commit `bcecc40`) — Z-Line không tự cài đặt lại giao thức.

> Client không chính thức. Zalo có thể hạn chế tài khoản dùng client bên thứ ba; hãy thử bằng tài khoản phụ.

## Trạng thái

Giai đoạn 1–2: lệnh CLI. Chế độ TUI kiểu Discordo (3 khung, realtime qua WebSocket, ảnh inline) là giai đoạn sau.

## Lệnh

```
zline login [--force]          đăng nhập bằng mã QR (vẽ ngay trong terminal)
zline whoami [--offline]       tài khoản và tình trạng phiên
zline sync                     tải danh sách bạn/nhóm và tin mới
zline sync <nhóm|cloud>        kéo lịch sử cũ (nhóm và Cloud)
zline ls [--groups|--friends] [--refresh] [-n N]
zline read <hội thoại> [-n 30]
zline send <hội thoại> "nội dung"      (không có nội dung hoặc "-" thì đọc stdin)
zline logout [--purge]
```

`<hội thoại>` nhận ID, tên đầy đủ, hoặc một phần tên (không phân biệt dấu). `cloud` hoặc `me` là Cloud của tôi. Nếu khớp nhiều hội thoại, Z-Line liệt kê ra thay vì đoán.

Tùy chọn chung: `--json`, `--profile <tên>`, `--data-dir <thư mục>`, `--proxy <url>`.

Ví dụ:

```
zline login
zline sync
zline ls --groups -n 10
echo "đã xong" | zline send "lop 12a1"
zline read "nguyen van an" -n 20 --json
```

## Dữ liệu

Mỗi hồ sơ có thư mục riêng: `<data home>/z-line/profiles/<profile>/` (`$XDG_DATA_HOME` hoặc `~/.local/share` trên Linux) chứa `session.json`, `threads_cache.json`, `zalo_history.db`. Không commit các file này. Đừng trỏ `--data-dir` vào thư mục đang được ctdozlite dùng cùng lúc: hai tiến trình ghi chung SQLite và `session.json` có thể xung đột.

## Build

```
cargo build --release     # binary: target/release/zline
cargo test
```

`zalo_rust` kéo theo `eframe`, `cpal` và `opus`, nên cần trên Linux: `pkg-config cmake libasound2-dev libopus-dev`.

## CI và phát hành

`.github/workflows/build.yml` chạy trên GitHub Actions:

- Mỗi push vào `main`, mỗi pull request và khi bấm **Run workflow**: chạy test và build cho Linux x86_64, Windows x86_64, macOS arm64 và macOS x86_64. Tải file ở mục **Artifacts** của lần chạy (giữ 30 ngày).
- Khi push tag dạng `v0.1.0`: build xong tự tạo GitHub Release, kèm `SHA256SUMS.txt`. Tag phải trùng `version` trong `Cargo.toml`, nếu không workflow dừng.

```
git tag v0.1.0 && git push origin v0.1.0
```

`.cargo/config.toml` và `packaging/macos/*.cmake` cần cho việc build libopus bằng CMake; đừng xóa.
