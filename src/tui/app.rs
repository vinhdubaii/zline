//! TUI state and key handling.
//!
//! Nothing here touches the terminal or the network: key handlers return [`Effect`]s for the
//! runtime to carry out, which keeps the whole interaction model unit-testable.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::ListState;
use zalo_rust::chat::{self, ChatMessage, Thread};
use zalo_rust::clock;

use super::editor::Editor;
use crate::threads;

/// Quick reactions on keys `1`..`6` (same set and order as ctdozlite's own TUI).
pub const QUICK_REACTIONS: [&str; 6] = [
    "\u{1F44D}",
    "\u{2764}\u{FE0F}",
    "\u{1F606}",
    "\u{1F62E}",
    "\u{1F622}",
    "\u{1F620}",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Tree,
    Messages,
    Input,
    Members,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Friends,
    Groups,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RowKey {
    Header(Section),
    Thread(String),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Conn {
    /// No usable session (not logged in, or the server refused it).
    Offline(String),
    /// Session present but not yet confirmed by the server.
    Connecting,
    Online,
}

#[derive(Clone, Debug)]
pub struct ThreadItem {
    pub id: String,
    pub name: String,
    pub is_group: bool,
    pub is_cloud: bool,
    pub last_ts: i64,
    pub preview: String,
    pub unread: u32,
    /// `(id, name)` pairs from the cached group roster.
    pub members: Vec<(String, String)>,
    pub member_count: Option<u64>,
}

impl ThreadItem {
    pub fn from_thread(t: &Thread, cloud_id: Option<&str>) -> Self {
        Self {
            id: t.id.clone(),
            name: t.name.clone(),
            is_group: t.is_group,
            is_cloud: cloud_id == Some(t.id.as_str()),
            last_ts: threads::last_ts(t),
            preview: threads::preview_of(t),
            unread: 0,
            members: t
                .presentation
                .members
                .iter()
                .map(|m| (m.id.clone(), m.name.clone()))
                .collect(),
            member_count: t.presentation.member_count,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MsgState {
    Sent,
    Sending,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct UiMsg {
    pub m: ChatMessage,
    pub state: MsgState,
}

pub struct Picker {
    pub query: String,
    pub cursor: usize,
}

#[derive(Debug)]
pub struct SendReq {
    pub thread_id: String,
    pub thread_name: String,
    pub is_group: bool,
    pub text: String,
    pub quote: Option<ChatMessage>,
    pub local_id: String,
}

#[derive(Debug)]
pub enum Effect {
    Quit,
    /// Clear and fully repaint the terminal (e.g. after stray output).
    Redraw,
    Refresh,
    /// Load stored messages for this thread, then call [`App::set_messages`].
    Open(String),
    Send(SendReq),
    React {
        thread_id: String,
        is_group: bool,
        msg: ChatMessage,
        emoji: &'static str,
    },
}

pub struct App {
    pub threads: Vec<ThreadItem>,
    pub cloud_id: Option<String>,
    pub me: String,
    pub conn: Conn,

    pub sel: Option<String>,
    pub msgs: Vec<UiMsg>,
    pub cursor: usize,
    pub stick_bottom: bool,
    /// First visible line of the message view; maintained by the renderer.
    pub top: usize,
    /// Height of the message view in lines; set by the renderer for paging.
    pub view_h: usize,

    pub focus: Focus,
    pub tree_cur: Option<RowKey>,
    pub tree_state: ListState,
    pub friends_open: bool,
    pub groups_open: bool,
    pub show_tree: bool,
    pub show_members: bool,
    pub members_scroll: usize,

    pub input: Editor,
    pub reply: Option<ChatMessage>,
    pub picker: Option<Picker>,
    pub help: bool,
    pub status: Option<(String, Instant)>,

    pub dirty: bool,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    next_local: u64,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl App {
    pub fn new(list: &[Thread], cloud_id: Option<String>, me: String, conn: Conn) -> Self {
        let mut threads: Vec<ThreadItem> = list
            .iter()
            .filter(|t| !t.id.is_empty() && t.id != "0")
            .map(|t| ThreadItem::from_thread(t, cloud_id.as_deref()))
            .collect();
        threads.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        let mut app = Self {
            threads,
            cloud_id,
            me,
            conn,
            sel: None,
            msgs: Vec::new(),
            cursor: 0,
            stick_bottom: true,
            top: 0,
            view_h: 10,
            focus: Focus::Tree,
            tree_cur: None,
            tree_state: ListState::default(),
            friends_open: true,
            groups_open: true,
            show_tree: true,
            show_members: true,
            members_scroll: 0,
            input: Editor::default(),
            reply: None,
            picker: None,
            help: false,
            status: None,
            dirty: true,
            seen: HashSet::new(),
            seen_order: VecDeque::new(),
            next_local: 0,
        };
        app.tree_cur = app.rows().into_iter().find(|r| matches!(r, RowKey::Thread(_)));
        app
    }

    // ---- queries -------------------------------------------------------------------------

    pub fn thread(&self, id: &str) -> Option<&ThreadItem> {
        self.threads.iter().find(|t| t.id == id)
    }

    pub fn selected(&self) -> Option<&ThreadItem> {
        self.sel.as_deref().and_then(|id| self.thread(id))
    }

    pub fn section_open(&self, s: Section) -> bool {
        match s {
            Section::Friends => self.friends_open,
            Section::Groups => self.groups_open,
        }
    }

    pub fn section_count(&self, s: Section) -> usize {
        let groups = s == Section::Groups;
        self.threads
            .iter()
            .filter(|t| !t.is_cloud && t.is_group == groups)
            .count()
    }

    /// Visible rows of the conversation tree, in display order.
    pub fn rows(&self) -> Vec<RowKey> {
        let mut rows: Vec<RowKey> = self
            .threads
            .iter()
            .filter(|t| t.is_cloud)
            .map(|t| RowKey::Thread(t.id.clone()))
            .collect();
        for (sec, groups) in [(Section::Friends, false), (Section::Groups, true)] {
            if self.section_count(sec) == 0 {
                continue;
            }
            rows.push(RowKey::Header(sec));
            if self.section_open(sec) {
                rows.extend(
                    self.threads
                        .iter()
                        .filter(|t| !t.is_cloud && t.is_group == groups)
                        .map(|t| RowKey::Thread(t.id.clone())),
                );
            }
        }
        rows
    }

    pub fn can_send(&self) -> bool {
        !matches!(self.conn, Conn::Offline(_))
    }

    pub fn picker_matches(&self) -> Vec<usize> {
        let Some(p) = &self.picker else {
            return Vec::new();
        };
        let q = threads::fold(&p.query);
        self.threads
            .iter()
            .enumerate()
            .filter(|(_, t)| q.is_empty() || threads::fold(&t.name).contains(&q))
            .map(|(i, _)| i)
            .take(100)
            .collect()
    }

    // ---- status / connection -------------------------------------------------------------

    pub fn set_status(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), Instant::now()));
        self.dirty = true;
    }

    pub fn set_conn(&mut self, c: Conn) {
        if self.conn != c {
            self.conn = c;
            self.dirty = true;
        }
    }

    pub fn on_tick(&mut self) {
        if let Some((_, at)) = &self.status {
            if at.elapsed() > Duration::from_secs(6) {
                self.status = None;
                self.dirty = true;
            }
        }
    }

    // ---- data updates --------------------------------------------------------------------

    /// Remember a message ID; returns `true` the first time it is seen.
    fn mark_seen(&mut self, id: &str) -> bool {
        if id.is_empty() {
            return true;
        }
        if !self.seen.insert(id.to_string()) {
            return false;
        }
        self.seen_order.push_back(id.to_string());
        while self.seen_order.len() > 5000 {
            if let Some(old) = self.seen_order.pop_front() {
                self.seen.remove(&old);
            }
        }
        true
    }

    /// Replace the open thread's messages with `msgs` (oldest first) loaded from storage.
    pub fn set_messages(&mut self, thread_id: &str, mut msgs: Vec<ChatMessage>) {
        if self.sel.as_deref() != Some(thread_id) {
            return;
        }
        msgs.sort_by_key(|m| clock::normalize_ts_ms(m.ts));
        for m in &msgs {
            self.mark_seen(&m.id);
        }
        self.msgs = msgs
            .into_iter()
            .map(|m| UiMsg {
                m,
                state: MsgState::Sent,
            })
            .collect();
        self.cursor = self.msgs.len().saturating_sub(1);
        self.stick_bottom = true;
        self.dirty = true;
    }

    fn upsert(&mut self, t: &Thread, live: bool) -> usize {
        if let Some(i) = self.threads.iter().position(|x| x.id == t.id) {
            let it = &mut self.threads[i];
            if !t.name.is_empty()
                && (chat::is_generic_thread_name(&it.name)
                    || !chat::is_generic_thread_name(&t.name))
            {
                it.name = t.name.clone();
            }
            it.is_group |= t.is_group;
            if !t.presentation.members.is_empty() {
                it.members = t
                    .presentation
                    .members
                    .iter()
                    .map(|m| (m.id.clone(), m.name.clone()))
                    .collect();
            }
            if t.presentation.member_count.is_some() {
                it.member_count = t.presentation.member_count;
            }
            i
        } else {
            let mut it = ThreadItem::from_thread(t, self.cloud_id.as_deref());
            if live {
                // A conversation we have never seen: everything in it is news.
                it.last_ts = 0;
            }
            self.threads.push(it);
            self.threads.len() - 1
        }
    }

    /// Merge conversations and messages that arrived from the network.
    ///
    /// `live` is true for realtime/poll deliveries and false for a full refresh. Unread counts only
    /// include incoming messages newer than what we already knew, so a replayed backlog does not
    /// light up old conversations.
    pub fn ingest(&mut self, incoming: Vec<Thread>, live: bool) {
        for t in incoming {
            if t.id.is_empty() || t.id == "0" {
                continue;
            }
            let idx = self.upsert(&t, live);
            let is_sel = self.sel.as_deref() == Some(t.id.as_str());
            let prev_last = self.threads[idx].last_ts;
            let mut msgs = t.messages;
            msgs.sort_by_key(|m| clock::normalize_ts_ms(m.ts));
            for m in msgs {
                let ts = clock::normalize_ts_ms(m.ts);
                let fresh = self.mark_seen(&m.id);
                let text = m.text.replace('\n', " ");
                let outgoing = m.is_outgoing;
                if is_sel {
                    self.push_live(m);
                } else if fresh && !outgoing && ts > prev_last {
                    self.threads[idx].unread += 1;
                }
                let item = &mut self.threads[idx];
                if ts >= item.last_ts {
                    item.last_ts = ts;
                    if !text.is_empty() {
                        item.preview = text;
                    }
                }
            }
        }
        self.threads.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        self.dirty = true;
    }

    /// Add a message to the open thread, merging with an unconfirmed local copy if there is one.
    fn push_live(&mut self, m: ChatMessage) {
        if !m.id.is_empty() && self.msgs.iter().any(|u| u.m.id == m.id) {
            return;
        }
        if m.is_outgoing {
            if let Some(p) = self
                .msgs
                .iter_mut()
                .find(|u| u.state == MsgState::Sending && u.m.text == m.text)
            {
                p.m = m;
                p.state = MsgState::Sent;
                return;
            }
        }
        let cur_id = self.msgs.get(self.cursor).map(|u| u.m.id.clone());
        self.msgs.push(UiMsg {
            m,
            state: MsgState::Sent,
        });
        let n = self.msgs.len();
        if n >= 2
            && clock::normalize_ts_ms(self.msgs[n - 2].m.ts)
                > clock::normalize_ts_ms(self.msgs[n - 1].m.ts)
        {
            self.msgs
                .sort_by_key(|u| clock::normalize_ts_ms(u.m.ts));
            if !self.stick_bottom {
                if let Some(id) = cur_id {
                    if let Some(p) = self.msgs.iter().position(|u| u.m.id == id) {
                        self.cursor = p;
                    }
                }
            }
        }
        if self.stick_bottom {
            self.cursor = self.msgs.len() - 1;
        }
        self.dirty = true;
    }

    pub fn on_send_done(&mut self, local_id: &str, server_id: &str) {
        if let Some(u) = self.msgs.iter_mut().find(|u| u.m.id == local_id) {
            u.state = MsgState::Sent;
            if !server_id.is_empty() {
                u.m.id = server_id.to_string();
            }
        }
        if !server_id.is_empty() {
            self.mark_seen(server_id);
        }
        self.dirty = true;
    }

    pub fn on_send_failed(&mut self, local_id: &str, err: &str) {
        if let Some(u) = self.msgs.iter_mut().find(|u| u.m.id == local_id) {
            u.state = MsgState::Failed(err.to_string());
        }
        self.set_status(format!("Gửi thất bại: {err}"));
    }

    // ---- key handling --------------------------------------------------------------------

    pub fn on_paste(&mut self, s: &str) {
        self.dirty = true;
        if let Some(p) = &mut self.picker {
            p.query.push_str(s.lines().next().unwrap_or(""));
            p.cursor = 0;
            return;
        }
        if self.sel.is_some() {
            self.focus = Focus::Input;
            self.input.insert_str(s);
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        self.dirty = true;
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);

        if self.help {
            self.help = false;
            return Vec::new();
        }
        if self.picker.is_some() {
            return self.picker_key(k);
        }

        match (k.code, ctrl) {
            (KeyCode::Char('c' | 'q'), true) => return vec![Effect::Quit],
            (KeyCode::Char('g'), true) => {
                self.show_tree = true;
                self.focus = Focus::Tree;
                return Vec::new();
            }
            (KeyCode::Char('t'), true) => {
                self.focus = Focus::Messages;
                return Vec::new();
            }
            (KeyCode::Char('y'), true) => {
                self.show_members = true;
                self.focus = Focus::Members;
                return Vec::new();
            }
            (KeyCode::Char('k'), true) => {
                self.picker = Some(Picker {
                    query: String::new(),
                    cursor: 0,
                });
                return Vec::new();
            }
            (KeyCode::Char('r'), true) => return vec![Effect::Refresh],
            (KeyCode::Char('l'), true) => return vec![Effect::Redraw],
            (KeyCode::F(1), _) => {
                self.help = true;
                return Vec::new();
            }
            (KeyCode::F(2), _) => {
                self.show_tree = !self.show_tree;
                if !self.show_tree && self.focus == Focus::Tree {
                    self.focus = Focus::Input;
                }
                return Vec::new();
            }
            (KeyCode::F(3), _) => {
                self.show_members = !self.show_members;
                if !self.show_members && self.focus == Focus::Members {
                    self.focus = Focus::Input;
                }
                return Vec::new();
            }
            (KeyCode::Tab, false) => {
                self.cycle_focus(true);
                return Vec::new();
            }
            (KeyCode::BackTab, _) => {
                self.cycle_focus(false);
                return Vec::new();
            }
            _ => {}
        }

        match self.focus {
            Focus::Tree => self.tree_key(k),
            Focus::Messages => self.messages_key(k),
            Focus::Input => self.input_key(k),
            Focus::Members => {
                self.members_key(k);
                Vec::new()
            }
        }
    }

    fn cycle_focus(&mut self, forward: bool) {
        let mut order = Vec::new();
        if self.show_tree {
            order.push(Focus::Tree);
        }
        order.push(Focus::Messages);
        order.push(Focus::Input);
        if self.show_members {
            order.push(Focus::Members);
        }
        let pos = order.iter().position(|f| *f == self.focus).unwrap_or(0);
        let n = order.len();
        let next = if forward { (pos + 1) % n } else { (pos + n - 1) % n };
        self.focus = order[next];
    }

    fn tree_move(&mut self, delta: isize) {
        let rows = self.rows();
        if rows.is_empty() {
            return;
        }
        let pos = self
            .tree_cur
            .as_ref()
            .and_then(|k| rows.iter().position(|r| r == k))
            .unwrap_or(0) as isize;
        let np = (pos + delta).clamp(0, rows.len() as isize - 1) as usize;
        self.tree_cur = Some(rows[np].clone());
    }

    fn toggle_section(&mut self, s: Section) {
        match s {
            Section::Friends => self.friends_open = !self.friends_open,
            Section::Groups => self.groups_open = !self.groups_open,
        }
    }

    fn tree_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.tree_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.tree_move(1),
            KeyCode::PageUp => self.tree_move(-10),
            KeyCode::PageDown => self.tree_move(10),
            KeyCode::Home | KeyCode::Char('g') => self.tree_move(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.tree_move(isize::MAX / 2),
            KeyCode::Left | KeyCode::Char('h') => {
                if let Some(RowKey::Header(s)) = self.tree_cur.clone() {
                    if self.section_open(s) {
                        self.toggle_section(s);
                    }
                }
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if let Some(RowKey::Header(s)) = self.tree_cur.clone() {
                    if !self.section_open(s) {
                        self.toggle_section(s);
                    }
                } else {
                    return self.tree_activate();
                }
            }
            KeyCode::Char(' ') => {
                if let Some(RowKey::Header(s)) = self.tree_cur.clone() {
                    self.toggle_section(s);
                }
            }
            KeyCode::Enter => return self.tree_activate(),
            KeyCode::Esc => self.focus = Focus::Input,
            _ => {}
        }
        Vec::new()
    }

    fn tree_activate(&mut self) -> Vec<Effect> {
        match self.tree_cur.clone() {
            Some(RowKey::Header(s)) => {
                self.toggle_section(s);
                Vec::new()
            }
            Some(RowKey::Thread(id)) => self.open_thread(&id),
            None => Vec::new(),
        }
    }

    pub fn open_thread(&mut self, id: &str) -> Vec<Effect> {
        let Some(item) = self.threads.iter_mut().find(|t| t.id == id) else {
            return Vec::new();
        };
        item.unread = 0;
        if item.is_group {
            self.groups_open = true;
        } else if !item.is_cloud {
            self.friends_open = true;
        }
        self.sel = Some(id.to_string());
        self.tree_cur = Some(RowKey::Thread(id.to_string()));
        self.msgs.clear();
        self.cursor = 0;
        self.stick_bottom = true;
        self.top = 0;
        self.reply = None;
        self.members_scroll = 0;
        self.focus = Focus::Input;
        vec![Effect::Open(id.to_string())]
    }

    fn msg_move(&mut self, delta: isize) {
        if self.msgs.is_empty() {
            return;
        }
        let last = self.msgs.len() - 1;
        self.cursor = (self.cursor as isize + delta).clamp(0, last as isize) as usize;
        self.stick_bottom = self.cursor == last;
    }

    fn messages_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => self.msg_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.msg_move(1),
            KeyCode::PageUp => self.msg_move(-5),
            KeyCode::PageDown => self.msg_move(5),
            KeyCode::Home | KeyCode::Char('g') => self.msg_move(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.msg_move(isize::MAX / 2),
            KeyCode::Char('i') | KeyCode::Esc => self.focus = Focus::Input,
            KeyCode::Char('r') | KeyCode::Enter => {
                if let Some(u) = self.msgs.get(self.cursor) {
                    if u.m.id.starts_with("local_") || u.state != MsgState::Sent {
                        self.set_status("Tin nhắn chưa gửi xong nên chưa thể trả lời");
                    } else {
                        self.reply = Some(u.m.clone());
                        self.focus = Focus::Input;
                    }
                }
            }
            KeyCode::Char(c @ '1'..='6') => {
                let idx = c as usize - '1' as usize;
                let (Some(u), Some(sel)) = (self.msgs.get(self.cursor), self.selected()) else {
                    return Vec::new();
                };
                if u.m.id.starts_with("local_") || u.state != MsgState::Sent {
                    self.set_status("Tin nhắn chưa gửi xong nên chưa thể thả cảm xúc");
                    return Vec::new();
                }
                if !self.can_send() {
                    self.set_status("Chưa đăng nhập: không thả cảm xúc được");
                    return Vec::new();
                }
                let emoji = QUICK_REACTIONS[idx];
                let fx = Effect::React {
                    thread_id: sel.id.clone(),
                    is_group: sel.is_group,
                    msg: u.m.clone(),
                    emoji,
                };
                self.set_status(format!("Đã gửi cảm xúc {emoji}"));
                return vec![fx];
            }
            _ => {}
        }
        Vec::new()
    }

    fn members_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.members_scroll = self.members_scroll.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') => self.members_scroll += 1,
            KeyCode::PageUp => self.members_scroll = self.members_scroll.saturating_sub(10),
            KeyCode::PageDown => self.members_scroll += 10,
            KeyCode::Esc => self.focus = Focus::Input,
            _ => {}
        }
    }

    fn input_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Enter if shift || alt => self.input.newline(),
            KeyCode::Enter => return self.submit(),
            KeyCode::Char('j') if ctrl => self.input.newline(),
            KeyCode::Char('w') if ctrl => self.input.delete_word_back(),
            KeyCode::Char('u') if ctrl => self.input.delete_to_line_start(),
            KeyCode::Char('a') if ctrl => self.input.home(),
            KeyCode::Char('e') if ctrl => self.input.end(),
            KeyCode::Char(c) if !ctrl && !alt => self.input.insert_char(c),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Down => {
                self.input.down();
            }
            KeyCode::Up => {
                // On the first line of the box, Up moves to the message list.
                if !self.input.up() {
                    self.focus = Focus::Messages;
                    if !self.msgs.is_empty() {
                        self.cursor = self.msgs.len() - 1;
                        self.stick_bottom = true;
                    }
                }
            }
            KeyCode::Esc => {
                if self.reply.is_some() {
                    self.reply = None;
                } else {
                    self.focus = Focus::Messages;
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn submit(&mut self) -> Vec<Effect> {
        let text = self.input.text();
        if text.trim().is_empty() {
            return Vec::new();
        }
        let Some(sel) = self.selected().cloned() else {
            self.set_status("Hãy chọn một hội thoại trước (Ctrl+G hoặc Ctrl+K)");
            return Vec::new();
        };
        if !self.can_send() {
            self.set_status("Chưa đăng nhập hoặc phiên hết hạn: chỉ xem được lịch sử (chạy `zline login`)");
            return Vec::new();
        }
        self.next_local += 1;
        let local_id = format!("local_{}", self.next_local);
        let ts = now_ms();
        let quote = self.reply.take();
        let mut m = ChatMessage {
            id: local_id.clone(),
            sender: self.me.clone(),
            text: text.clone(),
            ts,
            time: clock::message_time(ts),
            is_outgoing: true,
            ..Default::default()
        };
        if let Some(q) = &quote {
            m.reply_to_id = Some(q.id.clone());
            m.reply_to_sender = Some(q.sender.clone());
            m.reply_to_text = Some(q.text.clone());
        }
        self.msgs.push(UiMsg {
            m,
            state: MsgState::Sending,
        });
        self.cursor = self.msgs.len() - 1;
        self.stick_bottom = true;
        self.input.clear();
        if let Some(item) = self.threads.iter_mut().find(|t| t.id == sel.id) {
            item.last_ts = ts;
            item.preview = text.replace('\n', " ");
        }
        self.threads.sort_by(|a, b| b.last_ts.cmp(&a.last_ts));
        vec![Effect::Send(SendReq {
            thread_id: sel.id,
            thread_name: sel.name,
            is_group: sel.is_group,
            text,
            quote,
            local_id,
        })]
    }

    fn picker_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let matches = self.picker_matches();
        let Some(p) = self.picker.as_mut() else {
            return Vec::new();
        };
        match k.code {
            KeyCode::Esc => self.picker = None,
            KeyCode::Char('k') if ctrl => self.picker = None,
            KeyCode::Up => p.cursor = p.cursor.saturating_sub(1),
            KeyCode::Down => p.cursor = (p.cursor + 1).min(matches.len().saturating_sub(1)),
            KeyCode::Backspace => {
                p.query.pop();
                p.cursor = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                p.query.push(c);
                p.cursor = 0;
            }
            KeyCode::Enter => {
                let chosen = matches
                    .get(p.cursor)
                    .and_then(|i| self.threads.get(*i))
                    .map(|t| t.id.clone());
                self.picker = None;
                if let Some(id) = chosen {
                    return self.open_thread(&id);
                }
            }
            _ => {}
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..key(KeyCode::Char(c))
        }
    }

    fn th(id: &str, name: &str, is_group: bool, msgs: Vec<ChatMessage>) -> Thread {
        Thread {
            id: id.into(),
            name: name.into(),
            is_group,
            messages: msgs,
            ..Default::default()
        }
    }

    fn msg(id: &str, text: &str, ts: i64, outgoing: bool) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            sender: "An".into(),
            text: text.into(),
            ts,
            is_outgoing: outgoing,
            ..Default::default()
        }
    }

    fn app(conn: Conn) -> App {
        let list = vec![
            th("c", "Cloud của tôi (Lưu trữ)", false, vec![msg("x", "note", 1_000, true)]),
            th("u1", "Nguyễn Văn An", false, vec![msg("a", "chào", 1_500_000_000_000, false)]),
            th("u2", "Trần Bình", false, vec![msg("b", "hi", 1_400_000_000_000, false)]),
            th("g1", "Lớp 12A1", true, vec![msg("g", "họp", 1_600_000_000_000, false)]),
        ];
        App::new(&list, Some("c".into()), "Tôi".into(), conn)
    }

    fn type_text(a: &mut App, s: &str) {
        for c in s.chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn threads_sorted_recent_first_and_tree_groups_them() {
        let a = app(Conn::Online);
        let ids: Vec<_> = a.threads.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["g1", "u1", "u2", "c"]);
        let rows = a.rows();
        assert_eq!(rows[0], RowKey::Thread("c".into())); // Cloud is pinned on top
        assert_eq!(rows[1], RowKey::Header(Section::Friends));
        assert_eq!(rows[2], RowKey::Thread("u1".into()));
        assert_eq!(rows[4], RowKey::Header(Section::Groups));
        assert_eq!(rows[5], RowKey::Thread("g1".into()));
    }

    #[test]
    fn tree_navigation_opens_a_thread_and_focuses_input() {
        let mut a = app(Conn::Online);
        assert_eq!(a.focus, Focus::Tree);
        a.on_key(key(KeyCode::Down)); // Friends header
        a.on_key(key(KeyCode::Down)); // u1
        let fx = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&fx[..], [Effect::Open(id)] if id == "u1"));
        assert_eq!(a.sel.as_deref(), Some("u1"));
        assert_eq!(a.focus, Focus::Input);
    }

    #[test]
    fn header_toggles_with_space_and_hides_children() {
        let mut a = app(Conn::Online);
        a.on_key(key(KeyCode::Down)); // Friends header
        let before = a.rows().len();
        a.on_key(key(KeyCode::Char(' ')));
        assert_eq!(a.rows().len(), before - 2);
        a.on_key(key(KeyCode::Right));
        assert_eq!(a.rows().len(), before);
    }

    #[test]
    fn picker_filters_without_accents_and_opens() {
        let mut a = app(Conn::Online);
        a.on_key(ctrl('k'));
        type_text(&mut a, "nguyen");
        assert_eq!(a.picker_matches().len(), 1);
        let fx = a.on_key(key(KeyCode::Enter));
        assert!(matches!(&fx[..], [Effect::Open(id)] if id == "u1"));
        assert!(a.picker.is_none());
    }

    #[test]
    fn sending_adds_a_pending_message_and_emits_effect() {
        let mut a = app(Conn::Online);
        a.open_thread("u1");
        a.set_messages("u1", vec![msg("a", "chào", 1_500_000_000_000, false)]);
        type_text(&mut a, "xin chào");
        let fx = a.on_key(key(KeyCode::Enter));
        let Some(Effect::Send(req)) = fx.into_iter().next() else {
            panic!("expected a send effect")
        };
        assert_eq!(req.text, "xin chào");
        assert_eq!(req.thread_id, "u1");
        assert!(a.input.is_empty());
        let last = a.msgs.last().unwrap();
        assert_eq!(last.state, MsgState::Sending);
        assert!(last.m.id.starts_with("local_"));
        assert_eq!(a.threads[0].id, "u1"); // moved to the top
    }

    #[test]
    fn send_confirmation_and_echo_do_not_duplicate() {
        let mut a = app(Conn::Online);
        a.open_thread("u1");
        a.set_messages("u1", vec![]);
        type_text(&mut a, "hello");
        let Some(Effect::Send(req)) = a.on_key(key(KeyCode::Enter)).into_iter().next() else {
            panic!()
        };
        a.on_send_done(&req.local_id, "srv1");
        assert_eq!(a.msgs[0].m.id, "srv1");
        assert_eq!(a.msgs[0].state, MsgState::Sent);
        // The server echo of the same message arrives afterwards.
        a.ingest(vec![th("u1", "Nguyễn Văn An", false, vec![msg("srv1", "hello", 5, true)])], true);
        assert_eq!(a.msgs.len(), 1);
    }

    #[test]
    fn echo_before_confirmation_replaces_the_pending_copy() {
        let mut a = app(Conn::Online);
        a.open_thread("u1");
        a.set_messages("u1", vec![]);
        type_text(&mut a, "hello");
        let Some(Effect::Send(req)) = a.on_key(key(KeyCode::Enter)).into_iter().next() else {
            panic!()
        };
        a.ingest(vec![th("u1", "Nguyễn Văn An", false, vec![msg("srv1", "hello", 5, true)])], true);
        assert_eq!(a.msgs.len(), 1);
        assert_eq!(a.msgs[0].state, MsgState::Sent);
        a.on_send_done(&req.local_id, "srv1"); // late confirmation is harmless
        assert_eq!(a.msgs.len(), 1);
    }

    #[test]
    fn failed_send_is_marked_and_keeps_the_message() {
        let mut a = app(Conn::Online);
        a.open_thread("u1");
        a.set_messages("u1", vec![]);
        type_text(&mut a, "x");
        let Some(Effect::Send(req)) = a.on_key(key(KeyCode::Enter)).into_iter().next() else {
            panic!()
        };
        a.on_send_failed(&req.local_id, "mạng lỗi");
        assert!(matches!(a.msgs[0].state, MsgState::Failed(_)));
        assert!(a.status.is_some());
    }

    #[test]
    fn offline_blocks_sending_but_keeps_the_draft() {
        let mut a = app(Conn::Offline("chưa đăng nhập".into()));
        a.open_thread("u1");
        type_text(&mut a, "nháp");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        assert_eq!(a.input.text(), "nháp");
        assert!(a.status.is_some());
    }

    #[test]
    fn shift_enter_and_ctrl_j_insert_newlines() {
        let mut a = app(Conn::Online);
        a.open_thread("u1");
        type_text(&mut a, "a");
        a.on_key(KeyEvent {
            modifiers: KeyModifiers::SHIFT,
            ..key(KeyCode::Enter)
        });
        type_text(&mut a, "b");
        a.on_key(ctrl('j'));
        type_text(&mut a, "c");
        assert_eq!(a.input.text(), "a\nb\nc");
    }

    #[test]
    fn incoming_in_other_thread_counts_unread_only_for_newer_incoming_messages() {
        let mut a = app(Conn::Online);
        a.open_thread("g1");
        a.set_messages("g1", vec![]);
        // Older than what we already knew (replayed backlog): no badge.
        a.ingest(vec![th("u1", "Nguyễn Văn An", false, vec![msg("old", "cũ", 1, false)])], true);
        assert_eq!(a.thread("u1").unwrap().unread, 0);
        // Newer incoming message: badge. The same ID again: not counted twice.
        let newer = msg("new", "mới", 9_000_000_000_000, false);
        a.ingest(vec![th("u1", "Nguyễn Văn An", false, vec![newer.clone()])], true);
        a.ingest(vec![th("u1", "Nguyễn Văn An", false, vec![newer])], true);
        assert_eq!(a.thread("u1").unwrap().unread, 1);
        // Our own message never counts.
        a.ingest(vec![th("u1", "Nguyễn Văn An", false, vec![msg("mine", "tôi", 9_100_000_000_000, true)])], true);
        assert_eq!(a.thread("u1").unwrap().unread, 1);
        // Opening the thread clears it, and it moved to the top of the list.
        assert_eq!(a.threads[0].id, "u1");
        a.open_thread("u1");
        assert_eq!(a.thread("u1").unwrap().unread, 0);
    }

    #[test]
    fn brand_new_conversation_from_a_live_message_is_unread() {
        let mut a = app(Conn::Online);
        a.ingest(vec![th("u9", "Người dùng 9", false, vec![msg("n", "alo", 5_000_000_000_000, false)])], true);
        assert_eq!(a.thread("u9").unwrap().unread, 1);
        // A real name arriving later replaces the placeholder; a placeholder never replaces a real name.
        a.ingest(vec![th("u9", "Lê Cường", false, vec![])], true);
        assert_eq!(a.thread("u9").unwrap().name, "Lê Cường");
        a.ingest(vec![th("u9", "Người dùng 9", false, vec![])], true);
        assert_eq!(a.thread("u9").unwrap().name, "Lê Cường");
    }

    #[test]
    fn incoming_in_open_thread_follows_the_bottom() {
        let mut a = app(Conn::Online);
        a.open_thread("u1");
        a.set_messages("u1", vec![msg("a", "một", 100, false)]);
        a.ingest(vec![th("u1", "Nguyễn Văn An", false, vec![msg("b", "hai", 200, false)])], true);
        assert_eq!(a.msgs.len(), 2);
        assert_eq!(a.cursor, 1);
        assert_eq!(a.thread("u1").unwrap().unread, 0);
    }

    #[test]
    fn reply_sets_quote_and_is_sent_with_it() {
        let mut a = app(Conn::Online);
        a.open_thread("u1");
        a.set_messages("u1", vec![msg("a", "ăn tối chưa?", 100, false)]);
        a.on_key(key(KeyCode::Esc)); // input -> messages
        assert_eq!(a.focus, Focus::Messages);
        a.on_key(key(KeyCode::Char('r')));
        assert_eq!(a.focus, Focus::Input);
        assert!(a.reply.is_some());
        type_text(&mut a, "rồi");
        let Some(Effect::Send(req)) = a.on_key(key(KeyCode::Enter)).into_iter().next() else {
            panic!()
        };
        assert_eq!(req.quote.as_ref().map(|q| q.id.as_str()), Some("a"));
        assert!(a.reply.is_none());
        assert_eq!(a.msgs.last().unwrap().m.reply_to_text.as_deref(), Some("ăn tối chưa?"));
    }

    #[test]
    fn number_keys_react_to_the_selected_message() {
        let mut a = app(Conn::Online);
        a.open_thread("u1");
        a.set_messages("u1", vec![msg("a", "hi", 100, false)]);
        a.on_key(ctrl('t'));
        let fx = a.on_key(key(KeyCode::Char('2')));
        assert!(matches!(&fx[..], [Effect::React { emoji, .. }] if *emoji == QUICK_REACTIONS[1]));
    }

    #[test]
    fn cannot_react_or_reply_to_unsent_messages() {
        let mut a = app(Conn::Online);
        a.open_thread("u1");
        a.set_messages("u1", vec![]);
        type_text(&mut a, "x");
        a.on_key(key(KeyCode::Enter));
        a.on_key(ctrl('t'));
        assert!(a.on_key(key(KeyCode::Char('1'))).is_empty());
        a.on_key(key(KeyCode::Char('r')));
        assert!(a.reply.is_none());
    }

    #[test]
    fn message_cursor_tracks_bottom_stickiness() {
        let mut a = app(Conn::Online);
        a.open_thread("u1");
        a.set_messages("u1", (0..5).map(|i| msg(&format!("m{i}"), "x", i, false)).collect());
        a.on_key(ctrl('t'));
        assert!(a.stick_bottom);
        a.on_key(key(KeyCode::Up));
        assert!(!a.stick_bottom);
        a.on_key(key(KeyCode::Char('G')));
        assert!(a.stick_bottom);
        a.on_key(key(KeyCode::Char('g')));
        assert_eq!(a.cursor, 0);
    }

    #[test]
    fn focus_shortcuts_and_cycling() {
        let mut a = app(Conn::Online);
        a.on_key(ctrl('t'));
        assert_eq!(a.focus, Focus::Messages);
        a.on_key(ctrl('y'));
        assert_eq!(a.focus, Focus::Members);
        a.on_key(ctrl('g'));
        assert_eq!(a.focus, Focus::Tree);
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.focus, Focus::Messages);
        a.on_key(key(KeyCode::BackTab));
        assert_eq!(a.focus, Focus::Tree);
    }

    #[test]
    fn quit_keys() {
        let mut a = app(Conn::Online);
        assert!(matches!(&a.on_key(ctrl('c'))[..], [Effect::Quit]));
        assert!(matches!(&a.on_key(ctrl('q'))[..], [Effect::Quit]));
    }

    #[test]
    fn paste_goes_to_the_input_box() {
        let mut a = app(Conn::Online);
        a.open_thread("u1");
        a.on_key(ctrl('g'));
        a.on_paste("dòng 1\r\ndòng 2");
        assert_eq!(a.focus, Focus::Input);
        assert_eq!(a.input.text(), "dòng 1\ndòng 2");
    }
}
