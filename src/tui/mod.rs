//! Z-Line's terminal UI: a Discordo-style three-pane client (conversations, messages, members).

mod app;
mod data;
mod editor;
mod net;
mod text;
mod ui;

use std::io::{stdout, Stdout};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc::unbounded_channel;
use zalo_rust::session::SessionStatus;

use self::app::{App, Conn, Effect};
use self::net::{AppEvent, Tx};
use crate::cli::Global;
use crate::ctx::{Ctx, SESSION_FILE};
use crate::threads;

static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

/// Put the terminal back the way we found it. Safe to call more than once.
fn restore_terminal() {
    let mut out = stdout();
    if KEYBOARD_ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = execute!(out, DisableBracketedPaste, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

struct TermGuard;

impl Drop for TermGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

fn enter_terminal() -> Result<(Terminal<CrosstermBackend<Stdout>>, TermGuard)> {
    // A panic must not leave the shell in raw mode on the alternate screen.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));

    enable_raw_mode()?;
    let guard = TermGuard;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
    // With the kitty keyboard protocol (kitty, WezTerm, foot, ...) Shift+Enter is reported as
    // such; elsewhere Alt+Enter and Ctrl+J still insert a newline.
    if supports_keyboard_enhancement().unwrap_or(false)
        && execute!(
            out,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    {
        KEYBOARD_ENHANCED.store(true, Ordering::SeqCst);
    }
    let term = Terminal::new(CrosstermBackend::new(out))?;
    Ok((term, guard))
}

/// Apply one event. Returns `true` when the UI should quit.
fn handle(
    ev: AppEvent,
    app: &mut App,
    ctx: &mut Ctx,
    tx: &Tx,
    term: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<bool> {
    let effects = match ev {
        AppEvent::Term(Event::Key(k)) if k.kind != KeyEventKind::Release => app.on_key(k),
        AppEvent::Term(Event::Paste(s)) => {
            app.on_paste(&s);
            Vec::new()
        }
        AppEvent::Term(Event::Resize(..)) => {
            app.dirty = true;
            Vec::new()
        }
        AppEvent::Term(_) => Vec::new(),
        AppEvent::Tick => {
            app.on_tick();
            Vec::new()
        }
        AppEvent::Session(SessionStatus::Valid) => {
            app.set_conn(Conn::Online);
            Vec::new()
        }
        AppEvent::Session(SessionStatus::Unreachable) => {
            app.set_status("Không kết nối được máy chủ Zalo, đang thử lại…");
            Vec::new()
        }
        AppEvent::Session(SessionStatus::Expired) | AppEvent::Expired => {
            ctx.s.mark_expired();
            let _ = ctx.s.save_to_file(SESSION_FILE);
            app.set_conn(Conn::Offline("phiên hết hạn — chạy `zline login`".into()));
            Vec::new()
        }
        AppEvent::Incoming(ts) => {
            app.set_conn(Conn::Online);
            app.ingest(ts, true);
            Vec::new()
        }
        AppEvent::Refreshed(ts, warnings) => {
            app.set_conn(Conn::Online);
            app.ingest(ts, false);
            match warnings.first() {
                Some(w) => app.set_status(format!("! {w}")),
                None => app.set_status("Đã đồng bộ"),
            }
            Vec::new()
        }
        AppEvent::SendDone { local_id, server_id } => {
            app.on_send_done(&local_id, &server_id);
            Vec::new()
        }
        AppEvent::SendFailed { local_id, err } => {
            app.on_send_failed(&local_id, &err);
            Vec::new()
        }
        AppEvent::Status(s) => {
            app.set_status(s);
            Vec::new()
        }
    };

    for fx in effects {
        match fx {
            Effect::Quit => return Ok(true),
            Effect::Redraw => {
                term.clear()?;
                app.dirty = true;
            }
            Effect::Refresh => {
                if app.can_send() {
                    app.set_status("Đang đồng bộ…");
                    net::spawn_refresh(ctx.s.clone(), tx.clone());
                } else {
                    app.set_status("Chưa đăng nhập: không đồng bộ được");
                }
            }
            Effect::Open(id) => match data::load_messages(&id, 200) {
                Ok(msgs) => app.set_messages(&id, msgs),
                Err(e) => app.set_status(format!("Không đọc được lịch sử: {e:#}")),
            },
            Effect::Send(req) => net::spawn_send(ctx.s.clone(), tx.clone(), req),
            Effect::React {
                thread_id,
                is_group,
                msg,
                emoji,
            } => net::spawn_react(ctx.s.clone(), tx.clone(), thread_id, is_group, msg, emoji),
        }
    }
    Ok(false)
}

pub async fn run(g: &Global) -> Result<()> {
    let mut ctx = Ctx::local(g)?;

    let conn = if ctx.s.login.is_none() {
        Conn::Offline("chưa đăng nhập — chạy `zline login`".into())
    } else if ctx.s.is_expired_marked() {
        Conn::Offline("phiên hết hạn — chạy `zline login`".into())
    } else {
        Conn::Connecting
    };
    let mut list = threads::load_local();
    threads::ensure_cloud(&mut list, &ctx.s);
    let me = ctx
        .s
        .login
        .as_ref()
        .map(|l| l.display_name.clone())
        .unwrap_or_default();
    let mut app = App::new(&list, ctx.cloud_id().map(str::to_owned), me, conn);
    drop(list);

    let (mut term, _guard) = enter_terminal()?;
    let (tx, mut rx) = unbounded_channel::<AppEvent>();
    net::spawn_input(tx.clone());
    net::spawn_tick(tx.clone());
    if app.conn == Conn::Connecting {
        net::start_session(ctx.s.clone(), tx.clone());
    }

    loop {
        if app.dirty {
            app.dirty = false;
            term.draw(|f| ui::draw(f, &mut app))?;
        }
        let Some(first) = rx.recv().await else { break };
        let mut quit = handle(first, &mut app, &mut ctx, &tx, &mut term)?;
        // Drain whatever else is queued so one redraw covers a burst of events.
        while !quit {
            match rx.try_recv() {
                Ok(ev) => quit = handle(ev, &mut app, &mut ctx, &tx, &mut term)?,
                Err(_) => break,
            }
        }
        if quit {
            break;
        }
    }
    Ok(())
}
