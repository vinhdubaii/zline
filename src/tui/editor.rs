//! A small multi-line text editor for the message box.

use super::text::char_w;

#[derive(Default, Clone, Debug)]
pub struct Editor {
    buf: Vec<char>,
    cur: usize,
}

impl Editor {
    pub fn text(&self) -> String {
        self.buf.iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.cur = 0;
    }

    pub fn insert_char(&mut self, c: char) {
        if c.is_control() && c != '\n' {
            return;
        }
        self.buf.insert(self.cur, c);
        self.cur += 1;
    }

    /// Insert pasted text: line endings are normalised and other control characters dropped.
    pub fn insert_str(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        for c in s.chars() {
            self.insert_char(c);
        }
    }

    pub fn newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn backspace(&mut self) {
        if self.cur > 0 {
            self.cur -= 1;
            self.buf.remove(self.cur);
        }
    }

    pub fn delete(&mut self) {
        if self.cur < self.buf.len() {
            self.buf.remove(self.cur);
        }
    }

    pub fn left(&mut self) {
        self.cur = self.cur.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cur = (self.cur + 1).min(self.buf.len());
    }

    fn line_start(&self, pos: usize) -> usize {
        self.buf[..pos]
            .iter()
            .rposition(|&c| c == '\n')
            .map_or(0, |i| i + 1)
    }

    fn line_end(&self, pos: usize) -> usize {
        self.buf[pos..]
            .iter()
            .position(|&c| c == '\n')
            .map_or(self.buf.len(), |i| pos + i)
    }

    pub fn home(&mut self) {
        self.cur = self.line_start(self.cur);
    }

    pub fn end(&mut self) {
        self.cur = self.line_end(self.cur);
    }

    /// Move to the same column on the previous logical line. Returns `false` on the first line.
    pub fn up(&mut self) -> bool {
        let start = self.line_start(self.cur);
        if start == 0 {
            return false;
        }
        let col = self.cur - start;
        let prev_start = self.line_start(start - 1);
        let prev_len = (start - 1) - prev_start;
        self.cur = prev_start + col.min(prev_len);
        true
    }

    /// Move to the same column on the next logical line. Returns `false` on the last line.
    pub fn down(&mut self) -> bool {
        let end = self.line_end(self.cur);
        if end >= self.buf.len() {
            return false;
        }
        let col = self.cur - self.line_start(self.cur);
        let next_start = end + 1;
        let next_len = self.line_end(next_start) - next_start;
        self.cur = next_start + col.min(next_len);
        true
    }

    /// Delete the word before the cursor (Ctrl+W).
    pub fn delete_word_back(&mut self) {
        while self.cur > 0 && self.buf[self.cur - 1].is_whitespace() {
            self.cur -= 1;
            self.buf.remove(self.cur);
        }
        while self.cur > 0 && !self.buf[self.cur - 1].is_whitespace() {
            self.cur -= 1;
            self.buf.remove(self.cur);
        }
    }

    /// Delete from the start of the line to the cursor (Ctrl+U).
    pub fn delete_to_line_start(&mut self) {
        let start = self.line_start(self.cur);
        self.buf.drain(start..self.cur);
        self.cur = start;
    }

    /// Hard-wrap to `width` columns. Returns the rows and the cursor as `(row, col)`.
    pub fn layout(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(2);
        let mut rows = vec![String::new()];
        let (mut r, mut c) = (0usize, 0usize);
        let mut cursor = (0usize, 0usize);
        for i in 0..=self.buf.len() {
            if i == self.buf.len() {
                if i == self.cur {
                    if c >= width {
                        rows.push(String::new());
                        cursor = (r + 1, 0);
                    } else {
                        cursor = (r, c);
                    }
                }
                break;
            }
            let ch = self.buf[i];
            if ch == '\n' {
                if i == self.cur {
                    cursor = (r, c);
                }
                r += 1;
                c = 0;
                rows.push(String::new());
                continue;
            }
            let w = char_w(ch);
            if c + w > width && c > 0 {
                r += 1;
                c = 0;
                rows.push(String::new());
            }
            if i == self.cur {
                cursor = (r, c);
            }
            rows[r].push(ch);
            c += w;
        }
        (rows, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(s: &str) -> Editor {
        let mut e = Editor::default();
        e.insert_str(s);
        e
    }

    #[test]
    fn insert_and_edit() {
        let mut e = ed("xin chao");
        e.backspace();
        e.insert_char('o');
        e.left();
        e.delete();
        assert_eq!(e.text(), "xin cha");
    }

    #[test]
    fn paste_normalises_line_endings_and_drops_controls() {
        let e = ed("a\r\nb\rc\x1b[31m");
        assert_eq!(e.text(), "a\nb\nc[31m");
    }

    #[test]
    fn word_and_line_deletes() {
        let mut e = ed("một hai ba");
        e.delete_word_back();
        assert_eq!(e.text(), "một hai ");
        e.delete_to_line_start();
        assert_eq!(e.text(), "");
    }

    #[test]
    fn vertical_movement_keeps_column() {
        let mut e = ed("abcd\nxy\nlmnop");
        // cursor is at the end of "lmnop" (col 5)
        assert!(e.up());
        e.insert_char('|'); // clamped to end of "xy"
        assert_eq!(e.text(), "abcd\nxy|\nlmnop");
        assert!(e.up());
        assert!(!e.up());
    }

    #[test]
    fn layout_wraps_and_places_cursor() {
        let e = ed("abcdef");
        let (rows, cur) = e.layout(4);
        assert_eq!(rows, vec!["abcd", "ef"]);
        assert_eq!(cur, (1, 2));

        let e = ed("abcd");
        let (rows, cur) = e.layout(4);
        assert_eq!(rows, vec!["abcd", ""]);
        assert_eq!(cur, (1, 0));
    }

    #[test]
    fn layout_handles_newlines() {
        let e = ed("a\nbc");
        let (rows, cur) = e.layout(10);
        assert_eq!(rows, vec!["a", "bc"]);
        assert_eq!(cur, (1, 2));
    }
}
