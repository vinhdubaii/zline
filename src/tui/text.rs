//! Display-width aware text helpers shared by the message view and the input box.

use unicode_width::UnicodeWidthChar;

pub fn char_w(c: char) -> usize {
    c.width().unwrap_or(0)
}

pub fn str_w(s: &str) -> usize {
    s.chars().map(char_w).sum()
}

/// Word-wrap `text` to `width` display columns. Explicit newlines are kept and words longer than
/// a line are hard-broken. Always returns at least one (possibly empty) line.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for para in text.split('\n') {
        let para = para.replace('\t', "    ");
        let mut line = String::new();
        let mut lw = 0usize;
        for word in para.split(' ') {
            let ww = str_w(word);
            let sep = usize::from(lw > 0);
            if lw + sep + ww <= width {
                if sep == 1 {
                    line.push(' ');
                }
                line.push_str(word);
                lw += sep + ww;
                continue;
            }
            if lw > 0 {
                out.push(std::mem::take(&mut line));
                lw = 0;
            }
            if ww <= width {
                line.push_str(word);
                lw = ww;
            } else {
                for c in word.chars() {
                    let cw = char_w(c);
                    if lw + cw > width && lw > 0 {
                        out.push(std::mem::take(&mut line));
                        lw = 0;
                    }
                    line.push(c);
                    lw += cw;
                }
            }
        }
        out.push(line);
    }
    out
}

/// Cut to `max` display columns, appending `…` when something was removed.
pub fn truncate_w(s: &str, max: usize) -> String {
    if str_w(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_w(c);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_on_word_boundaries() {
        assert_eq!(wrap("xin chào các bạn", 8), vec!["xin chào", "các bạn"]);
    }

    #[test]
    fn keeps_explicit_newlines_and_blank_lines() {
        assert_eq!(wrap("a\n\nb", 10), vec!["a", "", "b"]);
        assert_eq!(wrap("", 10), vec![""]);
    }

    #[test]
    fn hard_breaks_long_words() {
        assert_eq!(wrap("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn counts_wide_characters_as_two_columns() {
        // Each CJK/emoji-like wide char takes 2 columns, so only two fit in 4.
        assert_eq!(wrap("日本語", 4), vec!["日本", "語"]);
        assert_eq!(str_w("日本"), 4);
    }

    #[test]
    fn truncates_by_display_width() {
        assert_eq!(truncate_w("hello world", 6), "hello…");
        assert_eq!(truncate_w("ngắn", 10), "ngắn");
    }
}
