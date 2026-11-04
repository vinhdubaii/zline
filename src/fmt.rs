//! Small text helpers for terminal output.

use zalo_rust::clock;

/// Drop control characters (ESC, BEL, …) except newline and tab.
///
/// Message text comes from other people; printing it raw would let a sender inject terminal escape
/// sequences into the reader's terminal.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect()
}

/// Collapse whitespace/newlines and cut to `max` characters, adding `…` when truncated.
pub fn clip(s: &str, max: usize) -> String {
    let one = sanitize(s).split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= max {
        one
    } else {
        let mut out: String = one.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Right-pad to `width` characters (Vietnamese text is single-width, so char count is enough).
pub fn pad(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - n))
    }
}

/// `dd/mm HH:MM` in the local timezone, or `-` when the timestamp is unknown.
pub fn short_time(ts: i64) -> String {
    clock::local_datetime(ts)
        .map(|d| d.format("%d/%m %H:%M").to_string())
        .unwrap_or_else(|| "-".into())
}

/// `dd/mm/yyyy`, used for day separators in `read`.
pub fn day(ts: i64) -> String {
    clock::local_datetime(ts)
        .map(|d| d.format("%d/%m/%Y").to_string())
        .unwrap_or_default()
}

/// `HH:MM`.
pub fn hm(ts: i64) -> String {
    clock::message_time(ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_collapses_whitespace_and_truncates() {
        assert_eq!(clip("a\n  b", 10), "a b");
        assert_eq!(clip("xin chào các bạn", 8), "xin chà…");
        assert_eq!(clip("ngắn", 10), "ngắn");
    }

    #[test]
    fn sanitize_strips_escape_sequences_but_keeps_layout() {
        assert_eq!(sanitize("a\x1b[31mb\x07c\nd\te"), "a[31mbc\nd\te");
        assert_eq!(clip("x\x1b]0;pwn\x07y", 20), "x]0;pwny");
    }

    #[test]
    fn pad_counts_chars_not_bytes() {
        assert_eq!(pad("đủ", 4), "đủ  ");
        assert_eq!(pad("abcdef", 3), "abcdef");
    }

    #[test]
    fn unknown_timestamp_is_dash() {
        assert_eq!(short_time(0), "-");
        assert_eq!(day(0), "");
    }
}
