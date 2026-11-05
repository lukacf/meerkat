//! Terminal ownership and the UI event loop.
//!
//! All crossterm types come from `ratatui::crossterm` so the backend and our
//! commands can never disagree on the crossterm version; the `crossterm` crate is
//! used directly only for its `event-stream` feature (`EventStream`).

use std::io::{self, Stdout, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures::StreamExt;
use meerkat_core::{AgentEvent, EventEnvelope, ScopedAgentEvent};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use tokio::sync::mpsc;

use super::render::draw;
use super::selection::osc52;
use super::{App, Effect, History, Msg};

/// Render tick interval (~30 fps).
pub const TICK: Duration = Duration::from_millis(33);

/// Whether keyboard-enhancement flags were pushed (so restore knows to pop them).
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);
/// Whether mouse capture is enabled.
static MOUSE_CAPTURED: AtomicBool = AtomicBool::new(false);
/// Whether the terminal is currently in TUI mode.
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: Once = Once::new();

/// Errors from the UI runtime.
#[derive(Debug, thiserror::Error)]
pub enum UiError {
    /// Terminal I/O failed.
    #[error("terminal I/O failed: {0}")]
    Io(#[from] io::Error),
    /// The integrator dropped the effect receiver.
    #[error("effect channel closed")]
    EffectChannelClosed,
}

/// Restore the terminal to cooked mode. Idempotent and safe to call from a panic hook.
fn restore_terminal() {
    if !TERMINAL_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = io::stdout();
    if KEYBOARD_ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    if MOUSE_CAPTURED.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, DisableMouseCapture);
    }
    let _ = execute!(
        out,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        ratatui::crossterm::cursor::Show
    );
    let _ = disable_raw_mode();
    let _ = out.flush();
}

/// RAII owner of the terminal's TUI mode (raw mode, alternate screen, paste, keyboard flags).
#[derive(Debug)]
pub struct TerminalGuard {
    _private: (),
}

impl TerminalGuard {
    /// Enter TUI mode. `mouse` enables mouse capture (on by default: wheel
    /// scrolling and click-to-expand; native selection then needs Shift).
    pub fn enter(mouse: bool) -> io::Result<Self> {
        enable_raw_mode()?;
        TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
        // From here on, `Drop` restores whatever was enabled if a later step fails.
        let guard = Self { _private: () };
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
        if matches!(supports_keyboard_enhancement(), Ok(true)) {
            execute!(
                out,
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                )
            )?;
            KEYBOARD_ENHANCED.store(true, Ordering::SeqCst);
        }
        if mouse {
            execute!(out, EnableMouseCapture)?;
            MOUSE_CAPTURED.store(true, Ordering::SeqCst);
        }
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Install (once) a panic hook that restores the terminal before delegating to the
/// previous hook, so panic messages are readable.
pub fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));
    });
}

/// Receivers feeding the UI loop.
#[derive(Debug)]
pub struct UiChannels {
    /// Primary agent stream events.
    pub agent_rx: mpsc::Receiver<EventEnvelope<AgentEvent>>,
    /// Scoped (sub-agent) stream events.
    pub scoped_rx: mpsc::Receiver<ScopedAgentEvent>,
    /// Integrator messages: escalations, diffs, approvals, turn completion, status, notices.
    pub ui_rx: mpsc::UnboundedReceiver<Msg>,
}

/// `msg` (from the integrator) preceded by every agent and scoped event
/// already buffered. The integrator sends `TurnFinished`, `InjectAck` and
/// `Recalled` after the events that led to them, but on a different channel;
/// applying the buffered events first keeps a turn's text from landing after
/// its summary (or in the next turn).
pub fn after_buffered_events(msg: Msg, channels: &mut UiChannels) -> Vec<Msg> {
    let mut batch = Vec::new();
    while let Ok(env) = channels.agent_rx.try_recv() {
        batch.push(Msg::Agent(env));
    }
    while let Ok(ev) = channels.scoped_rx.try_recv() {
        batch.push(Msg::Scoped(ev));
    }
    batch.push(msg);
    batch
}

fn translate(event: Event, mouse: bool) -> Option<Msg> {
    match event {
        // Windows reports Press and Release; enhanced terminals also report Repeat.
        Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
            Some(Msg::Key(key))
        }
        Event::Paste(text) => Some(Msg::Paste(text)),
        Event::Resize(w, h) => Some(Msg::Resize(w, h)),
        Event::Mouse(m) if mouse => Some(Msg::Mouse(m)),
        _ => None,
    }
}

/// Run the UI until the user quits.
///
/// Effects produced by [`App::update`] are forwarded on `effects`; [`Effect::Quit`]
/// is forwarded too and then ends the loop. Every submitted line is appended to
/// each of `histories` (e.g. the workspace and the global history file). The
/// terminal is restored on return, on error and on panic.
pub async fn run_ui(
    mut app: App,
    mut channels: UiChannels,
    effects: mpsc::UnboundedSender<Effect>,
    mouse: bool,
    mut histories: Vec<History>,
) -> Result<(), UiError> {
    install_panic_hook();
    let _guard = TerminalGuard::enter(mouse)?;
    let mut terminal: Terminal<CrosstermBackend<Stdout>> =
        Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;

    let mut events = crossterm::event::EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        if app.take_dirty() {
            terminal.draw(|frame| draw(frame, &mut app))?;
        }
        // Agent events first: integrator messages refer to events sent
        // before them (see `after_buffered_events`).
        let batch = tokio::select! {
            biased;
            Some(env) = channels.agent_rx.recv() => vec![Msg::Agent(env)],
            Some(ev) = channels.scoped_rx.recv() => vec![Msg::Scoped(ev)],
            Some(msg) = channels.ui_rx.recv() => after_buffered_events(msg, &mut channels),
            maybe = events.next() => match maybe {
                Some(Ok(event)) => match translate(event, mouse) {
                    Some(msg) => vec![msg],
                    None => continue,
                },
                Some(Err(err)) => return Err(UiError::Io(err)),
                None => {
                    // Input stream ended (terminal gone): tell the integrator to shut down.
                    let _ = effects.send(Effect::Quit);
                    return Ok(());
                }
            },
            _ = tick.tick() => vec![Msg::Tick],
        };
        for msg in batch {
            if apply(&mut app, msg, &effects, &mut histories)? {
                return Ok(());
            }
        }
    }
}

/// Put `text` on the system clipboard with OSC 52 (wrapped for tmux when
/// `TMUX` is set). Failures are logged: copying must never end the UI.
fn write_clipboard(text: &str) {
    let seq = osc52(text, std::env::var_os("TMUX").is_some());
    let mut out = io::stdout().lock();
    if let Err(err) = out.write_all(seq.as_bytes()).and_then(|()| out.flush()) {
        tracing::warn!(error = %err, "could not write the clipboard sequence");
    }
}

/// Apply one message, record submitted lines and forward the effects.
/// Returns whether the user quit.
fn apply(
    app: &mut App,
    msg: Msg,
    effects: &mpsc::UnboundedSender<Effect>,
    histories: &mut [History],
) -> Result<bool, UiError> {
    let effects_now = app.update(msg);
    if let Some(text) = app.take_clipboard_text() {
        write_clipboard(&text);
    }
    for entry in app.take_submitted() {
        for history in histories.iter_mut() {
            if let Err(err) = history.append(&entry) {
                tracing::warn!(
                    error = %err,
                    path = %history.path().display(),
                    "could not append to input history"
                );
            }
        }
    }
    for effect in effects_now {
        let quit = effect == Effect::Quit;
        if effects.send(effect).is_err() && !quit {
            return Err(UiError::EffectChannelClosed);
        }
        if quit {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::ui::{Block, StatusInfo, TurnOutcome, TurnSummary};
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventState, KeyModifiers};

    #[tokio::test]
    async fn turn_finished_waits_for_buffered_agent_events() {
        let (agent_tx, agent_rx) = mpsc::channel(16);
        let (_scoped_tx, scoped_rx) = mpsc::channel(16);
        let (ui_tx, ui_rx) = mpsc::unbounded_channel();
        let mut channels = UiChannels {
            agent_rx,
            scoped_rx,
            ui_rx,
        };
        let mut app = App::new(StatusInfo::default());
        for c in "hi".chars() {
            app.update(Msg::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::NONE,
            )));
        }
        app.update(Msg::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        // The turn's text is still buffered when its result arrives.
        for (seq, delta) in [(1, "Hel"), (2, "lo")] {
            agent_tx
                .send(EventEnvelope::new(
                    "s",
                    seq,
                    None,
                    AgentEvent::TextDelta {
                        delta: delta.into(),
                        assistant_message_id: None,
                    },
                ))
                .await
                .unwrap();
        }
        ui_tx
            .send(Msg::TurnFinished(TurnOutcome::plain(Ok(TurnSummary {
                text: "Hello".into(),
                input_tokens: 0,
                output_tokens: 0,
            }))))
            .unwrap();
        let msg = channels.ui_rx.recv().await.unwrap();
        let batch = after_buffered_events(msg, &mut channels);
        assert_eq!(batch.len(), 3);
        assert!(matches!(batch.last(), Some(Msg::TurnFinished(_))));
        for msg in batch {
            app.update(msg);
        }
        let assistant: Vec<&Block> = app
            .transcript
            .blocks()
            .iter()
            .filter(|b| matches!(b, Block::Assistant { .. }))
            .collect();
        assert_eq!(assistant, vec![&Block::Assistant { md: "Hello".into() }]);
        assert!(!app.is_running());
    }

    #[test]
    fn translate_filters_release_events() {
        let mut key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(matches!(
            translate(Event::Key(key), false),
            Some(Msg::Key(_))
        ));
        key.kind = KeyEventKind::Release;
        key.state = KeyEventState::NONE;
        assert!(translate(Event::Key(key), false).is_none());
        assert!(matches!(
            translate(Event::Paste("x".into()), false),
            Some(Msg::Paste(_))
        ));
        assert!(matches!(
            translate(Event::Resize(1, 2), false),
            Some(Msg::Resize(1, 2))
        ));
        assert!(translate(Event::FocusGained, false).is_none());
    }
}
