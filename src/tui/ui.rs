//! Rendering. Everything here reads [`App`]; the only mutations are the view-state fields the
//! renderer owns (scroll offset, viewport height, list state).

use ratatui::{
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState,
    },
    Frame,
};
use zalo_rust::chat::ChatMessage;
use zalo_rust::clock;
use zalo_rust::message_model::MessageKind;

use super::app::{App, Conn, Focus, MsgState, RowKey, Section};
use super::text::{str_w, truncate_w, wrap};
use crate::fmt::{day, hm, sanitize};

const ACCENT: Color = Color::Green;
const MUTED: Color = Color::DarkGray;
const SEL_BG: Color = Color::Indexed(237);
const NAMES: [Color; 6] = [
    Color::LightRed,
    Color::Yellow,
    Color::LightGreen,
    Color::Cyan,
    Color::LightMagenta,
    Color::LightBlue,
];

fn name_color(name: &str) -> Color {
    let h = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    NAMES[h as usize % NAMES.len()]
}

fn pane(title: String, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { ACCENT } else { MUTED }))
        .title(Span::styled(
            title,
            Style::default().fg(if focused { ACCENT } else { Color::Gray }),
        ))
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.width < 20 || area.height < 6 {
        f.render_widget(Paragraph::new("Terminal quá nhỏ"), area);
        return;
    }
    let [main, bar] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);

    if main.width < 70 {
        // Too narrow for three columns: show only the focused pane.
        match app.focus {
            Focus::Tree => draw_tree(f, main, app),
            Focus::Members => draw_members(f, main, app),
            _ => draw_center(f, main, app),
        }
    } else {
        let mut left_w = if app.show_tree { (main.width / 4).clamp(22, 34) } else { 0 };
        let mut right_w = if app.show_members { (main.width / 5).clamp(20, 26) } else { 0 };
        if main.width - left_w - right_w < 30 {
            right_w = 0;
        }
        if main.width - left_w - right_w < 30 {
            left_w = 0;
        }
        let center_w = main.width - left_w - right_w;
        if left_w > 0 {
            draw_tree(f, Rect::new(main.x, main.y, left_w, main.height), app);
        }
        draw_center(f, Rect::new(main.x + left_w, main.y, center_w, main.height), app);
        if right_w > 0 {
            draw_members(
                f,
                Rect::new(main.x + left_w + center_w, main.y, right_w, main.height),
                app,
            );
        }
    }
    draw_bar(f, bar, app);

    if app.help {
        draw_help(f, area);
    } else if app.picker.is_some() {
        draw_picker(f, area, app);
    }
}

// ---- conversation tree ---------------------------------------------------------------------

fn draw_tree(f: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Tree;
    let inner_w = area.width.saturating_sub(2) as usize;
    let rows = app.rows();
    let items: Vec<ListItem> = rows
        .iter()
        .map(|r| match r {
            RowKey::Header(s) => {
                let label = match s {
                    Section::Friends => "Tin nhắn riêng",
                    Section::Groups => "Nhóm",
                };
                let arrow = if app.section_open(*s) { "▾" } else { "▸" };
                ListItem::new(Line::from(Span::styled(
                    format!("{arrow} {label} ({})", app.section_count(*s)),
                    Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
                )))
            }
            RowKey::Thread(id) => {
                let Some(t) = app.thread(id) else {
                    return ListItem::new("");
                };
                let (indent, prefix) = if t.is_cloud {
                    ("", "* ")
                } else if t.is_group {
                    ("  ", "# ")
                } else {
                    ("  ", "")
                };
                let badge = if t.unread > 0 {
                    format!(" ({})", t.unread)
                } else {
                    String::new()
                };
                let budget = inner_w
                    .saturating_sub(indent.len() + prefix.len() + badge.len())
                    .max(1);
                let name = truncate_w(&sanitize(&t.name), budget);
                let style = if t.unread > 0 {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                } else if app.sel.as_deref() == Some(id.as_str()) {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::Gray)
                };
                let mut spans = vec![Span::styled(format!("{indent}{prefix}{name}"), style)];
                if !badge.is_empty() {
                    spans.push(Span::styled(
                        badge,
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ));
                }
                ListItem::new(Line::from(spans))
            }
        })
        .collect();
    let idx = app
        .tree_cur
        .as_ref()
        .and_then(|k| rows.iter().position(|r| r == k));
    app.tree_state.select(idx);
    let list = List::new(items)
        .block(pane(" Hội thoại ".into(), focused))
        .highlight_style(if focused {
            Style::default().bg(SEL_BG)
        } else {
            Style::default()
        });
    f.render_stateful_widget(list, area, &mut app.tree_state);
}

// ---- messages ------------------------------------------------------------------------------

/// Pick the first visible line so that the selected message stays on screen.
pub fn window(total: usize, h: usize, top: usize, sel: Option<(usize, usize)>, stick: bool) -> usize {
    let max_top = total.saturating_sub(h);
    if stick {
        return max_top;
    }
    let mut top = top.min(max_top);
    if let Some((s, e)) = sel {
        if s < top {
            top = s;
        } else if e > top + h {
            top = if e - s >= h { s } else { e - h };
        }
    }
    top.min(max_top)
}

fn sender_label(app: &App, m: &ChatMessage) -> String {
    if m.is_outgoing {
        return "Bạn".into();
    }
    let s = sanitize(&m.sender);
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        if let Some(t) = app.selected() {
            if let Some((_, n)) = t.members.iter().find(|(id, n)| *id == s && !n.is_empty()) {
                return sanitize(n);
            }
            if !t.is_group {
                return sanitize(&t.name);
            }
        }
    }
    if s.is_empty() {
        "?".into()
    } else {
        s
    }
}

fn body_text(m: &ChatMessage) -> String {
    let t = sanitize(&m.text);
    let mut out = if !t.trim().is_empty() {
        t
    } else if m.sticker_id.is_some() || matches!(m.kind, MessageKind::Sticker) {
        "[sticker]".into()
    } else {
        match m.kind {
            MessageKind::Photo => "[ảnh]",
            MessageKind::Video => "[video]",
            MessageKind::Voice => "[tin nhắn thoại]",
            MessageKind::File => "[tệp]",
            MessageKind::Gif => "[gif]",
            MessageKind::Location => "[vị trí]",
            MessageKind::Contact => "[danh thiếp]",
            MessageKind::Recall => "[tin nhắn đã thu hồi]",
            _ => "[tin nhắn]",
        }
        .into()
    };
    if let Some(u) = &m.link_url {
        let u = sanitize(u);
        if !out.contains(&u) {
            out.push('\n');
            if let Some(title) = &m.link_title {
                out.push_str(&sanitize(title));
                out.push(' ');
            }
            out.push_str(&u);
        }
    }
    out
}

fn reaction_line(m: &ChatMessage) -> Option<String> {
    let parts: Vec<String> = if !m.reaction_summaries.is_empty() {
        m.reaction_summaries
            .iter()
            .filter(|r| r.count > 0)
            .map(|r| format!("{} {}", sanitize(&r.emoji), r.count))
            .collect()
    } else {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for r in &m.reactions {
            match counts.iter_mut().find(|(e, _)| e == r) {
                Some((_, n)) => *n += 1,
                None => counts.push((r.clone(), 1)),
            }
        }
        counts
            .into_iter()
            .map(|(e, n)| format!("{} {n}", sanitize(&e)))
            .collect()
    };
    (!parts.is_empty()).then(|| parts.join("  "))
}

/// All message lines plus, for each message, its `[start, end)` line range.
fn build_lines(app: &App, w: usize, highlight: bool) -> (Vec<Line<'static>>, Vec<Option<(usize, usize)>>) {
    let muted = Style::default().fg(MUTED);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut ranges = vec![None; app.msgs.len()];
    let mut prev_day = String::new();
    let mut prev: Option<(String, i64)> = None;
    let mut just_sep = false;

    for (i, um) in app.msgs.iter().enumerate() {
        let m = &um.m;
        let d = day(m.ts);
        if !d.is_empty() && d != prev_day {
            if !lines.is_empty() {
                lines.push(Line::raw(""));
            }
            lines.push(Line::from(Span::styled(format!("── {d} ──"), muted)).centered());
            prev_day = d;
            prev = None;
            just_sep = true;
        }
        let who = sender_label(app, m);
        let ts = clock::normalize_ts_ms(m.ts);
        let same = m.reply_to_text.is_none()
            && matches!(&prev, Some((p, pt)) if *p == who && (ts - *pt).abs() < 300_000);
        if !same && !lines.is_empty() && !just_sep {
            lines.push(Line::raw(""));
        }
        just_sep = false;
        let start = lines.len();

        if !same {
            let color = if m.is_outgoing { ACCENT } else { name_color(&who) };
            lines.push(Line::from(vec![
                Span::styled(who.clone(), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(format!("  {}", hm(m.ts)), muted),
            ]));
        }
        if let Some(rt) = &m.reply_to_text {
            let who_q = m
                .reply_to_sender
                .as_deref()
                .map(sanitize)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "?".into());
            let one = sanitize(rt).split_whitespace().collect::<Vec<_>>().join(" ");
            lines.push(Line::from(Span::styled(
                truncate_w(&format!("> {who_q}: {one}"), w),
                muted,
            )));
        }
        let mut body = wrap(&body_text(m), w);
        if um.state == MsgState::Sending {
            if let Some(last) = body.last_mut() {
                last.push_str(" …");
            }
        }
        let body_style = if um.state == MsgState::Sending {
            muted
        } else {
            Style::default()
        };
        lines.extend(body.into_iter().map(|l| Line::from(Span::styled(l, body_style))));
        if let Some(r) = reaction_line(m) {
            lines.push(Line::from(Span::styled(
                truncate_w(&r, w),
                Style::default().fg(Color::Yellow),
            )));
        }
        if let MsgState::Failed(e) = &um.state {
            lines.push(Line::from(Span::styled(
                truncate_w(&format!("✗ chưa gửi được: {}", sanitize(e)), w),
                Style::default().fg(Color::Red),
            )));
        }
        let end = lines.len();
        ranges[i] = Some((start, end));
        if highlight && i == app.cursor {
            for l in &mut lines[start..end] {
                *l = std::mem::take(l).patch_style(Style::default().bg(SEL_BG));
            }
        }
        prev = Some((who, ts));
    }
    (lines, ranges)
}

fn draw_center(f: &mut Frame, area: Rect, app: &mut App) {
    let in_w = area.width.saturating_sub(2) as usize;
    let (rows, cursor) = app.input.layout(in_w);
    let shown = rows.len().clamp(1, 6) as u16;
    let in_h = (shown + 2).min(area.height.saturating_sub(3)).max(3);
    let [msg_area, in_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(in_h)]).areas(area);
    draw_messages(f, msg_area, app);
    draw_input(f, in_area, app, &rows, cursor);
}

fn draw_messages(f: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Messages;
    let title = match app.selected() {
        Some(t) => format!(
            " {} ",
            truncate_w(&sanitize(&t.name), area.width.saturating_sub(6) as usize)
        ),
        None => " Z-Line ".into(),
    };
    let block = pane(title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 4 || inner.height < 1 {
        return;
    }
    app.view_h = inner.height as usize;
    let muted = Style::default().fg(MUTED);

    if app.sel.is_none() {
        let mut text = vec![
            Line::raw(""),
            Line::from(Span::styled("  Chọn một hội thoại để bắt đầu.", Style::default().fg(Color::Gray))),
            Line::from(Span::styled("  Ctrl+G: danh sách · Ctrl+K: chuyển nhanh · F1: trợ giúp", muted)),
        ];
        if let Conn::Offline(r) = &app.conn {
            text.push(Line::raw(""));
            text.push(Line::from(Span::styled(
                format!("  Đang offline ({r}): chỉ xem được lịch sử đã lưu."),
                Style::default().fg(Color::Yellow),
            )));
        }
        f.render_widget(Paragraph::new(text), inner);
        return;
    }
    if app.msgs.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  Chưa có tin nhắn lưu cục bộ. Ctrl+R để đồng bộ.",
                muted,
            ))),
            inner,
        );
        return;
    }

    let w = (inner.width as usize).saturating_sub(1).max(1);
    let (lines, ranges) = build_lines(app, w, focused);
    let total = lines.len();
    let h = inner.height as usize;
    let sel = ranges.get(app.cursor).copied().flatten();
    app.top = window(total, h, app.top, sel, app.stick_bottom);

    let mut visible: Vec<Line> = Vec::with_capacity(h);
    if total < h {
        visible.extend((0..h - total).map(|_| Line::raw("")));
    }
    let start = app.top.min(total);
    visible.extend(lines[start..(start + h).min(total)].iter().cloned());
    f.render_widget(
        Paragraph::new(visible),
        Rect::new(inner.x, inner.y, inner.width - 1, inner.height),
    );
    if total > h {
        let mut sb = ScrollbarState::new(total - h).position(app.top);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            inner,
            &mut sb,
        );
    }
}

fn draw_input(f: &mut Frame, area: Rect, app: &App, rows: &[String], cursor: (usize, usize)) {
    let focused = app.focus == Focus::Input;
    let title = match &app.reply {
        Some(q) => {
            let who = if q.is_outgoing { "Bạn".to_string() } else { sanitize(&q.sender) };
            let one = sanitize(&q.text).split_whitespace().collect::<Vec<_>>().join(" ");
            format!(" ↩ Trả lời {who}: {} (Esc: hủy) ", truncate_w(&one, 36))
        }
        None => " Tin nhắn ".into(),
    };
    let block = pane(title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let vis = inner.height as usize;
    let start = cursor.0.saturating_sub(vis - 1);
    if app.input.is_empty() {
        let hint = if app.can_send() {
            "Nhắn tin…"
        } else {
            "Chưa đăng nhập: chỉ xem được lịch sử (chạy `zline login`)"
        };
        f.render_widget(
            Paragraph::new(Span::styled(hint, Style::default().fg(MUTED))),
            inner,
        );
    } else {
        let lines: Vec<Line> = rows[start..]
            .iter()
            .take(vis)
            .map(|r| Line::raw(r.clone()))
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
    if focused && !app.help && app.picker.is_none() {
        let x = inner.x + (cursor.1 as u16).min(inner.width - 1);
        let y = inner.y + (cursor.0 - start) as u16;
        f.set_cursor_position(Position::new(x, y));
    }
}

// ---- members -------------------------------------------------------------------------------

fn draw_members(f: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Members;
    let muted = Style::default().fg(MUTED);
    let mut lines: Vec<Line> = Vec::new();
    let mut title = " Thông tin ".to_string();

    match app.selected() {
        None => lines.push(Line::from(Span::styled("—", muted))),
        Some(t) if t.is_cloud => {
            lines.push(Line::from("Cloud của tôi"));
            lines.push(Line::from(Span::styled("Lưu tin nhắn, ghi chú cho riêng bạn", muted)));
        }
        Some(t) if t.is_group => {
            // Only claim a count when the server gave one or we hold a roster.
            let known = t.member_count.or((!t.members.is_empty()).then_some(t.members.len() as u64));
            title = match known {
                Some(n) => format!(" Thành viên — {n} "),
                None => " Thành viên ".to_string(),
            };
            if t.members.is_empty() {
                lines.push(Line::from(Span::styled("Chưa có danh sách.", muted)));
                lines.push(Line::from(Span::styled("Ctrl+R để đồng bộ.", muted)));
            }
            for (id, name) in &t.members {
                let label = if name.is_empty() { id.clone() } else { name.clone() };
                let label = sanitize(&label);
                lines.push(Line::from(Span::styled(
                    truncate_w(&label, area.width.saturating_sub(4) as usize),
                    Style::default().fg(name_color(&label)),
                )));
            }
            if let Some(c) = t.member_count {
                if (c as usize) > t.members.len() && !t.members.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("… và {} người khác", c as usize - t.members.len()),
                        muted,
                    )));
                }
            }
        }
        Some(t) => {
            lines.push(Line::from(sanitize(&t.name)));
            lines.push(Line::from(Span::styled(format!("ID {}", sanitize(&t.id)), muted)));
            lines.push(Line::from(Span::styled("Hội thoại 1-1", muted)));
        }
    }

    let inner_h = area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(inner_h);
    app.members_scroll = app.members_scroll.min(max_scroll);
    let shown: Vec<Line> = lines.into_iter().skip(app.members_scroll).collect();
    f.render_widget(Paragraph::new(shown).block(pane(title, focused)), area);
}

// ---- help bar and overlays -----------------------------------------------------------------

fn hints(focus: Focus) -> &'static [(&'static str, &'static str)] {
    match focus {
        Focus::Input => &[
            ("enter", "gửi"),
            ("shift+enter", "xuống dòng"),
            ("esc", "hủy"),
            ("↑", "tin nhắn"),
            ("^g", "hội thoại"),
            ("^t", "tin nhắn"),
            ("^y", "thành viên"),
            ("^k", "chuyển nhanh"),
            ("^r", "đồng bộ"),
            ("F1", "trợ giúp"),
        ],
        Focus::Tree => &[
            ("↑↓", "chọn"),
            ("enter", "mở"),
            ("space", "thu/mở"),
            ("tab", "khung kế"),
            ("^k", "chuyển nhanh"),
            ("^r", "đồng bộ"),
            ("F1", "trợ giúp"),
        ],
        Focus::Messages => &[
            ("↑↓", "chọn"),
            ("r", "trả lời"),
            ("1-6", "cảm xúc"),
            ("g/G", "đầu/cuối"),
            ("i", "nhập"),
            ("^g", "hội thoại"),
            ("^k", "chuyển nhanh"),
            ("F1", "trợ giúp"),
        ],
        Focus::Members => &[("↑↓", "cuộn"), ("esc", "về ô nhập"), ("tab", "khung kế"), ("F1", "trợ giúp")],
    }
}

fn draw_bar(f: &mut Frame, area: Rect, app: &App) {
    let (conn_txt, conn_color) = match &app.conn {
        Conn::Online => ("● online".to_string(), Color::Green),
        Conn::Connecting => ("◌ đang kết nối".to_string(), Color::Yellow),
        Conn::Offline(_) => ("○ offline".to_string(), Color::Red),
    };
    let max_right = (area.width as usize * 3 / 5).max(10);
    let conn_txt = truncate_w(&conn_txt, max_right);
    let mut right: Vec<Span> = Vec::new();
    let mut right_w = str_w(&conn_txt);
    if let Some((s, _)) = &app.status {
        let avail = max_right.saturating_sub(right_w + 2);
        if avail > 4 {
            let s = truncate_w(&sanitize(s).replace('\n', " "), avail);
            right_w += str_w(&s) + 2;
            right.push(Span::styled(s, Style::default().fg(Color::White)));
            right.push(Span::raw("  "));
        }
    }
    right.push(Span::styled(conn_txt, Style::default().fg(conn_color)));
    let right_w = (right_w + 1).min(area.width as usize);
    let left_w = (area.width as usize).saturating_sub(right_w);

    let mut spans: Vec<Span> = Vec::new();
    let mut used = 0usize;
    for (i, (k, d)) in hints(app.focus).iter().enumerate() {
        let sep = if i == 0 { 0 } else { 3 };
        let need = sep + str_w(k) + 1 + str_w(d);
        if used + need > left_w {
            break;
        }
        if sep > 0 {
            spans.push(Span::styled(" • ", Style::default().fg(MUTED)));
        }
        spans.push(Span::styled(
            (*k).to_string(),
            Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {d}"), Style::default().fg(MUTED)));
        used += need;
    }
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(area.x, area.y, left_w as u16, 1),
    );
    f.render_widget(
        Paragraph::new(Line::from(right)).right_aligned(),
        Rect::new(area.x + left_w as u16, area.y, right_w as u16, 1),
    );
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

fn draw_help(f: &mut Frame, area: Rect) {
    let rows: [(&str, &str); 14] = [
        ("Ctrl+G / T / Y", "tới hội thoại / tin nhắn / thành viên"),
        ("Tab, Shift+Tab", "khung kế, khung trước"),
        ("Ctrl+K", "chuyển nhanh giữa hội thoại"),
        ("F2 / F3", "ẩn hiện hội thoại / thành viên"),
        ("↑↓ j k", "chọn trong danh sách"),
        ("Enter", "mở hội thoại; gửi tin"),
        ("Shift+Enter, Ctrl+J", "xuống dòng trong ô nhập"),
        ("r, Enter", "trả lời tin đang chọn"),
        ("1-6", "cảm xúc nhanh cho tin đang chọn"),
        ("g / G", "tin đầu / tin cuối"),
        ("Esc", "hủy trả lời, về khung trước"),
        ("Ctrl+W / Ctrl+U", "xóa từ / xóa tới đầu dòng"),
        ("Ctrl+R / Ctrl+L", "đồng bộ / vẽ lại màn hình"),
        ("Ctrl+C", "thoát"),
    ];
    let lines: Vec<Line> = rows
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(
                    format!("{k:<22}"),
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Span::raw((*d).to_string()),
            ])
        })
        .collect();
    let r = centered(area, 70, rows.len() as u16 + 4);
    f.render_widget(Clear, r);
    f.render_widget(
        Paragraph::new(lines).block(pane(" Phím tắt (phím bất kỳ để đóng) ".into(), true)),
        r,
    );
}

fn draw_picker(f: &mut Frame, area: Rect, app: &App) {
    let Some(p) = &app.picker else { return };
    let matches = app.picker_matches();
    let h = (matches.len() as u16 + 3).clamp(4, 16);
    let r = centered(area, 60, h);
    f.render_widget(Clear, r);
    let block = pane(" Chuyển nhanh ".into(), true);
    let inner = block.inner(r);
    f.render_widget(block, r);
    if inner.height < 2 {
        return;
    }
    let q = Rect::new(inner.x, inner.y, inner.width, 1);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("› ", Style::default().fg(ACCENT)),
            Span::raw(p.query.clone()),
        ])),
        q,
    );
    let list_area = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    let items: Vec<ListItem> = matches
        .iter()
        .filter_map(|i| app.threads.get(*i))
        .map(|t| {
            let kind = if t.is_cloud {
                "*"
            } else if t.is_group {
                "#"
            } else {
                " "
            };
            let badge = if t.unread > 0 { format!(" ({})", t.unread) } else { String::new() };
            let name = truncate_w(&sanitize(&t.name), (inner.width as usize).saturating_sub(4 + badge.len()));
            ListItem::new(Line::from(vec![
                Span::styled(format!("{kind} {name}"), Style::default().fg(Color::Gray)),
                Span::styled(badge, Style::default().fg(Color::Yellow)),
            ]))
        })
        .collect();
    let mut st = ListState::default().with_selected(Some(p.cursor.min(matches.len().saturating_sub(1))));
    f.render_stateful_widget(
        List::new(items).highlight_style(Style::default().bg(SEL_BG)),
        list_area,
        &mut st,
    );
    f.set_cursor_position(Position::new(
        (inner.x + 2 + str_w(&p.query) as u16).min(inner.x + inner.width.saturating_sub(1)),
        inner.y,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_follows_bottom_when_stuck() {
        assert_eq!(window(100, 10, 0, None, true), 90);
        assert_eq!(window(5, 10, 0, None, true), 0);
    }

    #[test]
    fn window_scrolls_up_to_reveal_selection() {
        assert_eq!(window(100, 10, 50, Some((40, 43)), false), 40);
    }

    #[test]
    fn window_scrolls_down_to_reveal_selection() {
        assert_eq!(window(100, 10, 50, Some((58, 63)), false), 53);
    }

    #[test]
    fn window_keeps_start_of_a_block_taller_than_the_view() {
        assert_eq!(window(100, 10, 0, Some((20, 45)), false), 20);
    }

    #[test]
    fn window_never_scrolls_past_the_end() {
        assert_eq!(window(20, 10, 99, None, false), 10);
    }

    #[test]
    fn name_color_is_stable() {
        assert_eq!(name_color("An"), name_color("An"));
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::tui::app::Conn;
    use ratatui::{backend::TestBackend, Terminal};
    use zalo_rust::chat::Thread;

    fn screen(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            let mut line = String::new();
            for x in 0..buf.area.width {
                line.push_str(buf[(x, y)].symbol());
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }

    fn msg(id: &str, who: &str, text: &str, ts: i64, outgoing: bool) -> ChatMessage {
        ChatMessage {
            id: id.into(),
            sender: who.into(),
            text: text.into(),
            ts,
            is_outgoing: outgoing,
            ..Default::default()
        }
    }

    fn demo_app(conn: Conn) -> App {
        let base = 1_793_000_000_000i64;
        let mut group = Thread {
            id: "g1".into(),
            name: "Lớp 12A1".into(),
            is_group: true,
            ..Default::default()
        };
        group.presentation.members = vec![
            zalo_rust::chat::SidebarMember { id: "1".into(), name: "Lan".into(), avatar: None },
            zalo_rust::chat::SidebarMember { id: "2".into(), name: "Hùng".into(), avatar: None },
        ];
        group.presentation.member_count = Some(31);
        let list = vec![
            Thread { id: "c".into(), name: "Cloud của tôi (Lưu trữ)".into(), ..Default::default() },
            Thread { id: "u1".into(), name: "Nguyễn Văn An".into(), ..Default::default() },
            group,
        ];
        let mut a = App::new(&list, Some("c".into()), "Tôi".into(), conn);
        a.open_thread("g1");
        let mut reply = msg("m3", "Hùng", "ok mình đi", base + 120_000, false);
        reply.reply_to_sender = Some("Lan".into());
        reply.reply_to_text = Some("tối nay họp lúc 8 giờ nhé cả lớp".into());
        let mut reacted = msg("m4", "Lan", "nhớ mang theo laptop", base + 180_000, false);
        reacted.reactions = vec!["👍".into(), "👍".into(), "❤️".into()];
        a.set_messages(
            "g1",
            vec![
                msg("m1", "Lan", "chào cả lớp, tối nay họp lúc 8 giờ nhé", base, false),
                msg("m2", "Lan", "dòng thứ hai của cùng một người gửi", base + 30_000, false),
                reply,
                msg("m5", "0", "mình xác nhận tham gia\nvà sẽ đến sớm 10 phút", base + 150_000, true),
                reacted,
                msg("m6", "Hùng", "\u{1b}]0;PWNED\u{7}đỏ \u{1b}[31mbình thường", base + 200_000, false),
            ],
        );
        a.ingest(
            vec![Thread {
                id: "u1".into(),
                name: "Nguyễn Văn An".into(),
                messages: vec![msg("n1", "Nguyễn Văn An", "ơi", base + 500_000_000, false)],
                ..Default::default()
            }],
            true,
        );
        a.set_status("Đã đồng bộ");
        a
    }

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        screen(&term)
    }

    #[test]
    fn three_pane_layout_shows_all_the_pieces() {
        let mut a = demo_app(Conn::Online);
        let s = render(&mut a, 110, 30);
        println!("{s}");
        for needle in [
            "Hội thoại",
            "Tin nhắn riêng (1)",
            "# Lớp 12A1",
            "Nguyễn Văn An (1)", // unread badge
            "Thành viên — 31",
            "Lan",
            "Hùng",
            "... và",
            "Lan  ",
            "> Lan: tối nay họp lúc 8 giờ nhé cả lớp",
            "Bạn",
            "● online",
            "Đã đồng bộ",
            "enter gửi",
        ] {
            // the "…" is a single char; accept either form in the assertion
            let n = needle.replace("...", "…");
            assert!(s.contains(&n), "missing {n:?} in:\n{s}");
        }
        // Wide emoji take two buffer cells, so check the reaction line piecewise.
        let line = s.lines().find(|l| l.contains('👍')).expect("reaction line");
        assert!(line.contains('2') && line.contains('❤') && line.contains('1'), "{line}");
    }

    #[test]
    fn terminal_escape_sequences_in_messages_are_not_rendered() {
        let mut a = demo_app(Conn::Online);
        let s = render(&mut a, 110, 30);
        assert!(!s.contains('\u{1b}'));
        assert!(!s.contains('\u{7}'));
        assert!(s.contains("đỏ"));
    }

    #[test]
    fn consecutive_messages_from_one_sender_share_a_header() {
        let mut a = demo_app(Conn::Online);
        let s = render(&mut a, 110, 40);
        let lan_headers = s.lines().filter(|l| l.contains("│Lan  ") || l.contains("Lan  ") && l.contains(':')).count();
        // "Lan" appears as a header twice at most (m1 and m4), not three times.
        assert!(s.matches("dòng thứ hai").count() == 1);
        assert!(lan_headers <= 3, "{s}");
    }

    #[test]
    fn narrow_terminal_shows_only_the_focused_pane() {
        let mut a = demo_app(Conn::Online);
        let s = render(&mut a, 60, 20);
        println!("{s}");
        assert!(s.contains("Lớp 12A1"));
        assert!(!s.contains("Thành viên"));
        a.on_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('g'),
            crossterm::event::KeyModifiers::CONTROL,
        ));
        let s = render(&mut a, 60, 20);
        assert!(s.contains("Hội thoại"));
        assert!(s.contains("Tin nhắn riêng"));
    }

    #[test]
    fn offline_state_is_visible_and_explained() {
        let mut a = demo_app(Conn::Offline("chưa đăng nhập".into()));
        a.input.clear();
        let s = render(&mut a, 110, 30);
        assert!(s.contains("○ offline"));
        assert!(s.contains("chỉ xem được lịch sử"));
    }

    #[test]
    fn overlays_render() {
        let mut a = demo_app(Conn::Online);
        a.help = true;
        let s = render(&mut a, 110, 30);
        assert!(s.contains("Phím tắt"));
        a.help = false;
        a.picker = Some(crate::tui::app::Picker { query: "lop".into(), cursor: 0 });
        let s = render(&mut a, 110, 30);
        println!("{s}");
        assert!(s.contains("Chuyển nhanh"));
        assert!(s.contains("› lop"));
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut a = demo_app(Conn::Online);
        for (w, h) in [(20, 6), (25, 8), (40, 10), (71, 12)] {
            let _ = render(&mut a, w, h);
        }
        let s = render(&mut a, 30, 4);
        assert!(s.contains("quá nhỏ"));
        let _ = render(&mut a, 10, 3); // must not panic either
    }

    #[test]
    fn long_history_scrolls_to_the_newest_message() {
        let mut a = demo_app(Conn::Online);
        let base = 1_793_000_000_000i64;
        let many: Vec<ChatMessage> = (0..200)
            .map(|i| msg(&format!("h{i}"), "Lan", &format!("tin số {i}"), base + i * 60_000, false))
            .collect();
        a.set_messages("g1", many);
        let s = render(&mut a, 110, 30);
        assert!(s.contains("tin số 199"));
        assert!(!s.contains("tin số 5\n"));
    }
}

#[cfg(test)]
mod bar_tests {
    use super::*;
    use crate::tui::app::Conn;
    use ratatui::{backend::TestBackend, Terminal};
    use zalo_rust::chat::Thread;

    #[test]
    fn offline_label_stays_short_so_hints_still_fit() {
        let list = vec![Thread { id: "u".into(), name: "An".into(), ..Default::default() }];
        let mut a = App::new(&list, None, "Tôi".into(), Conn::Offline("chưa đăng nhập — chạy `zline login`".into()));
        let mut term = Terminal::new(TestBackend::new(120, 20)).unwrap();
        term.draw(|f| draw(f, &mut a)).unwrap();
        let buf = term.backend().buffer();
        let bar: String = (0..buf.area.width).map(|x| buf[(x, 19)].symbol().to_string()).collect();
        assert!(bar.contains("○ offline"));
        assert!(bar.contains("chuyển nhanh"), "{bar}");
    }

    #[test]
    fn group_without_roster_does_not_claim_zero_members() {
        let list = vec![Thread { id: "g".into(), name: "Nhóm A".into(), is_group: true, ..Default::default() }];
        let mut a = App::new(&list, None, "Tôi".into(), Conn::Online);
        a.open_thread("g");
        let mut term = Terminal::new(TestBackend::new(110, 20)).unwrap();
        term.draw(|f| draw(f, &mut a)).unwrap();
        let buf = term.backend().buffer();
        let top: String = (0..buf.area.width).map(|x| buf[(x, 0)].symbol().to_string()).collect();
        assert!(top.contains("Thành viên"));
        assert!(!top.contains("— 0"), "{top}");
    }
}
