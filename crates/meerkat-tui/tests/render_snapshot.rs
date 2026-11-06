#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Render the UI into a `TestBackend` and assert on specific rows.

use std::time::Instant;

use meerkat_core::{AgentEvent, EventEnvelope, ToolCallArguments};
use meerkat_tui::ui::render::draw_at;
use meerkat_tui::ui::{
    App, Block, DiffView, EscalationId, EscalationPrompt, Msg, StatusInfo, ToolState,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use serde_json::json;

const W: u16 = 60;
const H: u16 = 12;

fn rows(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            let mut row = String::new();
            for x in 0..area.width {
                row.push_str(buffer[(x, y)].symbol());
            }
            row.trim_end().to_string()
        })
        .collect()
}

fn agent(ev: AgentEvent) -> Msg {
    Msg::Agent(EventEnvelope::new("s", 0, None, ev))
}

fn sample_app() -> App {
    let mut status = StatusInfo::default();
    status.set_model("anthropic", "claude-sonnet-4-6");
    status.set_session("abc123ef-0000");
    status.set_tokens(1200, 345);
    let mut app = App::new(status);
    app.transcript.push(Block::User {
        text: "list files".into(),
    });
    app.update(agent(AgentEvent::TurnStarted {
        turn_number: 0,
        assistant_message_id: None,
    }));
    app.update(agent(AgentEvent::TextDelta {
        delta: "Sure, let me look.".into(),
        assistant_message_id: None,
    }));
    app.update(agent(AgentEvent::ToolCallRequested {
        id: "t1".into(),
        name: "glob".into(),
        args: ToolCallArguments::from_value(json!({"pattern": "*.rs"})).unwrap(),
    }));
    app.update(agent(AgentEvent::ToolExecutionStarted {
        id: "t1".into(),
        name: "glob".into(),
    }));
    app
}

fn render(app: &mut App, now: Instant) -> Vec<String> {
    render_sized(app, now, W)
}

fn render_sized(app: &mut App, now: Instant, width: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, H)).unwrap();
    terminal.draw(|f| draw_at(f, app, now)).unwrap();
    rows(terminal.backend().buffer())
}

#[test]
fn renders_blocks_input_and_status_bar() {
    let mut app = sample_app();
    let rows = render(&mut app, Instant::now());
    assert_eq!(rows.len(), usize::from(H));
    // Transcript is short, so it starts at the top.
    assert_eq!(rows[0], "› list files");
    assert_eq!(rows[1], "");
    assert_eq!(rows[2], "• Sure, let me look.");
    assert_eq!(rows[3], "");
    assert_eq!(rows[4], "◐ glob({\"pattern\":\"*.rs\"}) running…");
    // Input box: top border then the prompt line.
    assert!(rows[9].starts_with("────"), "border row: {:?}", rows[9]);
    assert_eq!(rows[10], "›");
    assert_eq!(
        rows[11],
        " anthropic/claude-sonnet-4-6 │ session abc123ef │ ↑1.2k ↓345"
    );
}

#[test]
fn running_turn_shows_spinner_and_queue_in_status() {
    let mut app = sample_app();
    let now = Instant::now();
    for c in "go".chars() {
        app.update(Msg::Key(ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Char(c),
        )));
    }
    app.update_at(
        Msg::Key(ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Enter,
        )),
        now,
    );
    let rows = render_sized(&mut app, now, 100);
    assert!(
        rows[11].ends_with("⠋ running 0s"),
        "status row: {:?}",
        rows[11]
    );
    assert!(
        rows[9].contains("Enter queues"),
        "input title: {:?}",
        rows[9]
    );
}

#[test]
fn long_transcript_follows_bottom_and_scrolls_back() {
    let mut app = sample_app();
    for i in 0..20 {
        app.transcript.push(Block::User {
            text: format!("prompt {i}"),
        });
    }
    let now = Instant::now();
    let rows = render(&mut app, now);
    // 9 transcript rows; last block's spacer is the bottom row.
    assert_eq!(rows[7], "› prompt 19");
    assert_eq!(rows[8], "");
    app.update(Msg::Key(ratatui::crossterm::event::KeyEvent::from(
        ratatui::crossterm::event::KeyCode::PageUp,
    )));
    let rows = render(&mut app, now);
    // PgUp moves by viewport height - 2 = 7 lines.
    assert_eq!(rows[0], "› prompt 12");
    assert_eq!(rows[8], "› prompt 16");
    assert!(!app.scroll.follow);
}

#[test]
fn a_long_input_line_wraps_and_the_transcript_shrinks() {
    let mut app = sample_app();
    for i in 0..20 {
        app.transcript.push(Block::User {
            text: format!("prompt {i}"),
        });
    }
    for c in "abcdefghijklmnopqrstuvwxy".chars() {
        app.update(Msg::Key(ratatui::crossterm::event::KeyEvent::from(
            ratatui::crossterm::event::KeyCode::Char(c),
        )));
    }
    let mut terminal = Terminal::new(TestBackend::new(20, H)).unwrap();
    terminal
        .draw(|f| draw_at(f, &mut app, Instant::now()))
        .unwrap();
    let rows = rows(terminal.backend().buffer());
    // 18 columns after the prompt prefix: the line takes two rows, so the
    // transcript has 8 rows (one fewer than with a one-row input).
    assert_eq!(rows[6], "› prompt 19");
    assert_eq!(rows[7], "");
    assert!(rows[8].starts_with('─'), "{:?}", rows[8]);
    assert_eq!(rows[9], "› abcdefghijklmnopqr");
    assert_eq!(rows[10], "  stuvwxy");
    // The cursor is on the second row, after its 7 chars.
    assert_eq!(
        terminal.get_cursor_position().unwrap(),
        ratatui::layout::Position::new(9, 10)
    );
    // Mouse hit-testing sees the shorter transcript.
    assert!(app.block_at_row(7).is_some());
    assert!(app.block_at_row(8).is_none());
}

#[test]
fn escalation_modal_is_drawn_over_the_transcript() {
    let mut app = sample_app();
    app.update(Msg::Escalation(
        EscalationPrompt {
            agent: Some("researcher".into()),
            tool: "shell".into(),
            args_preview: "rm -rf build".into(),
            hint: None,
            reason: None,
        },
        EscalationId(1),
    ));
    let rows = render(&mut app, Instant::now());
    let joined = rows.join("\n");
    assert!(joined.contains("Approval needed"), "{joined}");
    assert!(joined.contains("Helper: researcher"), "{joined}");
    assert!(joined.contains("Tool: shell"), "{joined}");
    assert!(joined.contains("rm -rf build"), "{joined}");
    assert!(joined.contains("[y] allow once"), "{joined}");
}

fn render_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|f| draw_at(f, app, Instant::now())).unwrap();
    terminal.backend().buffer().clone()
}

#[test]
fn diff_block_snapshot() {
    let mut app = App::new(StatusInfo::default());
    app.transcript.push(Block::Tool {
        name: "edit_file".into(),
        args: json!({"path": "src/a.rs"}),
        state: ToolState::Done {
            is_error: false,
            duration_ms: 4,
        },
        result_preview: None,
        diff: Some(DiffView {
            path: "src/a.rs".into(),
            unified: "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@\n fn a() {}\n-let x = 1;\n+let x = 2;\n"
                .into(),
            added: 1,
            removed: 1,
        }),
        note: None,
        depth: 0,
        collapsed: false,
    });
    let buf = render_buffer(&mut app, W, 14);
    let rows = rows(&buf);
    assert_eq!(rows[0], "✓ edit_file({\"path\":\"src/a.rs\"}) (4ms) ▾");
    assert_eq!(rows[1], "  src/a.rs (+1 -1)");
    assert_eq!(rows[2], "  --- a/src/a.rs");
    assert_eq!(rows[3], "  +++ b/src/a.rs");
    assert_eq!(rows[4], "  @@ -1,2 +1,2 @@");
    assert_eq!(rows[5], "   fn a() {}");
    assert_eq!(rows[6], "  -let x = 1;");
    assert_eq!(rows[7], "  +let x = 2;");
    assert_eq!(buf[(2, 6)].fg, Color::Red);
    assert_eq!(buf[(2, 7)].fg, Color::Green);
    assert_eq!(buf[(2, 4)].fg, Color::Cyan);
    assert!(buf[(2, 2)].modifier.contains(Modifier::DIM));
}

#[test]
fn markdown_assistant_block_snapshot() {
    let mut app = App::new(StatusInfo::default());
    app.update(agent(AgentEvent::TurnStarted {
        turn_number: 0,
        assistant_message_id: None,
    }));
    app.update(agent(AgentEvent::TextDelta {
        delta: "## Plan\n\nUse **bold** and `code` with a [link](https://e.x).\n\n- one\n  - two\n\n```sh\ncargo   test\n"
            .into(), assistant_message_id: None }));
    let buf = render_buffer(&mut app, 40, 16);
    let rows = rows(&buf);
    assert_eq!(rows[0], "• Plan");
    assert_eq!(rows[1], "");
    assert_eq!(rows[2], "  Use bold and code with a link");
    assert_eq!(rows[3], "  (https://e.x).");
    assert_eq!(rows[4], "");
    assert_eq!(rows[5], "  • one");
    assert_eq!(rows[6], "    ◦ two");
    assert_eq!(rows[7], "");
    // The fence is still open (streaming): code renders verbatim with a gutter.
    assert_eq!(rows[8], "  ▎ cargo   test");
    // Heading is bold cyan; `bold` is bold; `code` is yellow.
    assert!(buf[(2, 0)].modifier.contains(Modifier::BOLD));
    assert_eq!(buf[(2, 0)].fg, Color::Cyan);
    assert!(buf[(6, 2)].modifier.contains(Modifier::BOLD));
    assert_eq!(buf[(15, 2)].fg, Color::Yellow);
}

#[test]
fn history_search_prompt_line() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = App::new(StatusInfo::default())
        .with_history(vec!["cargo build --release".into(), "git status".into()]);
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Char('r'),
        KeyModifiers::CONTROL,
    )));
    for c in "build".chars() {
        app.update(Msg::Key(KeyEvent::from(KeyCode::Char(c))));
    }
    let buf = render_buffer(&mut app, W, H);
    let lines = rows(&buf);
    assert!(lines[9].contains("Ctrl-R older"), "title: {:?}", lines[9]);
    assert_eq!(
        lines[10],
        "(reverse-i-search)`build': cargo build --release"
    );
    // The matched substring is highlighted; the rest of the entry is not.
    let at = "(reverse-i-search)`build': cargo ".chars().count() as u16;
    assert_eq!(buf[(at, 10)].bg, Color::Yellow);
    assert_eq!(buf[(at + 4, 10)].bg, Color::Yellow);
    assert_ne!(buf[(at + 5, 10)].bg, Color::Yellow);
    assert_ne!(buf[(at - 1, 10)].bg, Color::Yellow);
    for c in "zz".chars() {
        app.update(Msg::Key(KeyEvent::from(KeyCode::Char(c))));
    }
    let lines = rows(&render_buffer(&mut app, W, H));
    assert_eq!(lines[10], "(failing reverse-i-search)`buildzz':");
}

#[test]
fn queued_block_status_count_and_hints() {
    use meerkat_tui::delivery::{PendingId, SteerFailure};
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let mut app = sample_app();
    let now = Instant::now();
    for c in "go".chars() {
        app.update(Msg::Key(KeyEvent::from(KeyCode::Char(c))));
    }
    app.update_at(Msg::Key(KeyEvent::from(KeyCode::Enter)), now);
    for c in "also fix tests".chars() {
        app.update(Msg::Key(KeyEvent::from(KeyCode::Char(c))));
    }
    app.update_at(Msg::Key(KeyEvent::from(KeyCode::Enter)), now);
    let rows = render_sized(&mut app, now, 100);
    assert!(
        rows.iter().any(|r| r == "› also fix tests  · queued"),
        "rows: {rows:#?}"
    );
    // Steered into the runtime: not recallable, Esc interrupts.
    assert!(
        rows[9]
            .contains("Enter queues · Ctrl-X Ctrl-S interrupts and sends · Esc/Ctrl-C interrupts"),
        "input title: {:?}",
        rows[9]
    );
    assert!(
        rows[11].ends_with("│ queued 1"),
        "status row: {:?}",
        rows[11]
    );

    app.update(Msg::SteerAck {
        id: PendingId(1),
        result: Ok(()),
    });
    let rows = render_sized(&mut app, now, 100);
    assert!(
        rows.iter().any(|r| r == "› also fix tests  · delivering…"),
        "rows: {rows:#?}"
    );
    // The placeholder is dim.
    let buffer = render_buffer(&mut app, 100, H);
    let y = (0..H)
        .find(|&y| buffer[(0, y)].symbol() == "›" && buffer[(2, y)].symbol() == "a")
        .expect("queued row");
    assert!(buffer[(2, y)].modifier.contains(Modifier::DIM));

    // A message the UI holds can be recalled.
    for c in "and docs".chars() {
        app.update(Msg::Key(KeyEvent::from(KeyCode::Char(c))));
    }
    app.update_at(Msg::Key(KeyEvent::from(KeyCode::Enter)), now);
    app.update(Msg::SteerAck {
        id: PendingId(2),
        result: Err(SteerFailure::NotRunning),
    });
    let rows = render_sized(&mut app, now, 100);
    assert!(
        rows[9].contains("Esc recall all · ↑ edit last queued · Ctrl-X Ctrl-S send now"),
        "input title: {:?}",
        rows[9]
    );
}

#[test]
fn status_bar_shows_context_gauge_with_threshold_colours() {
    use meerkat_tui::ui::StatusUpdate;
    let mut app = sample_app();
    let now = Instant::now();
    app.update(Msg::Status(StatusUpdate::ContextWindow(Some(200_000))));
    app.update(Msg::Status(StatusUpdate::Context {
        used: Some(84_000),
        approx: false,
    }));
    let rows = render_sized(&mut app, now, 100);
    assert_eq!(
        rows[11],
        " anthropic/claude-sonnet-4-6 │ session abc123ef │ ↑1.2k ↓345 │ ctx 42% (84k/200k)"
    );

    app.update(Msg::Status(StatusUpdate::Context {
        used: Some(84_000),
        approx: true,
    }));
    let rows = render_sized(&mut app, now, 100);
    assert!(
        rows[11].ends_with("│ ctx ~42% (84k/200k)"),
        "status row: {:?}",
        rows[11]
    );

    // Colour of the gauge cell (`c` of `ctx`) at each threshold.
    let gauge_fg = |app: &mut App| {
        let buffer = render_buffer(app, 100, H);
        let x = (0..98)
            .find(|&x| {
                (0..3)
                    .map(|i| buffer[(x + i, H - 1)].symbol())
                    .eq(["c", "t", "x"])
            })
            .expect("ctx segment");
        buffer[(x, H - 1)].fg
    };
    let normal = gauge_fg(&mut app);
    assert_ne!(normal, Color::Yellow);
    assert_ne!(normal, Color::Red);
    for (used, colour) in [(140_000, Color::Yellow), (180_000, Color::Red)] {
        app.update(Msg::Status(StatusUpdate::Context {
            used: Some(used),
            approx: false,
        }));
        assert_eq!(gauge_fg(&mut app), colour, "used {used}");
    }

    // Window unknown: plain usage; nothing at all when usage is unknown.
    app.update(Msg::Status(StatusUpdate::ContextWindow(None)));
    let rows = render_sized(&mut app, now, 100);
    assert!(
        rows[11].ends_with("│ ctx 180k"),
        "status row: {:?}",
        rows[11]
    );
    app.update(Msg::Status(StatusUpdate::Context {
        used: None,
        approx: false,
    }));
    let rows = render_sized(&mut app, now, 100);
    assert!(!rows[11].contains("ctx"), "status row: {:?}", rows[11]);

    // A narrow terminal clips the bar instead of wrapping it.
    app.update(Msg::Status(StatusUpdate::ContextWindow(Some(200_000))));
    app.update(Msg::Status(StatusUpdate::Context {
        used: Some(84_000),
        approx: false,
    }));
    let rows = render_sized(&mut app, now, 40);
    assert_eq!(rows.len(), usize::from(H));
    assert!(rows[11].chars().count() <= 40);
}

#[test]
fn model_picker_snapshot() {
    use meerkat_tui::endpoints::WireDialect;
    use meerkat_tui::models::{ModelEntry, ModelGroup, ModelSource};
    let entry = |id: &str, name: &str, window: Option<u64>, source| ModelEntry {
        id: id.into(),
        display_name: Some(name.into()),
        source,
        context_window: window,
        max_output_tokens: None,
    };
    let mut status = StatusInfo::default();
    status.set_model("anthropic", "claude-sonnet-4-6");
    let mut app = App::new(status);
    app.update(Msg::ShowModelPicker(vec![
        ModelGroup {
            endpoint: "anthropic".into(),
            dialect: WireDialect::Anthropic,
            base_url: None,
            models: vec![
                entry(
                    "claude-sonnet-4-6",
                    "Sonnet 4.6",
                    Some(1_000_000),
                    ModelSource::Current,
                ),
                entry(
                    "claude-opus-4-6",
                    "Opus 4.6",
                    Some(200_000),
                    ModelSource::Provider,
                ),
                entry("claude-haiku-4-5", "Haiku 4.5", None, ModelSource::Provider),
            ],
            warning: None,
        },
        ModelGroup {
            endpoint: "litellm".into(),
            dialect: WireDialect::OpenAi,
            base_url: Some("http://localhost:4000".into()),
            models: Vec::new(),
            warning: Some("HTTP 500".into()),
        },
    ]));
    app.update(Msg::Key(ratatui::crossterm::event::KeyEvent::from(
        ratatui::crossterm::event::KeyCode::Down,
    )));
    let mut terminal = Terminal::new(TestBackend::new(72, H)).unwrap();
    terminal
        .draw(|f| draw_at(f, &mut app, Instant::now()))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let all = rows(buffer);
    let modal: Vec<&str> = all[1..9].iter().map(String::as_str).collect();
    assert_eq!(
        modal,
        vec![
            "  ┌ Select model ────────────────────────────────────────────────────┐",
            "  │filter: (type to filter)                                          │",
            "  │── anthropic (anthropic dialect) ──                               │",
            "  │claude-sonnet-4-6  Sonnet 4.6  1M ctx  current                    │",
            "  │claude-opus-4-6    Opus 4.6  200k ctx                             │",
            "  │claude-haiku-4-5   Haiku 4.5                                      │",
            "  │── litellm (openai dialect, http://localhost:4000) ──             │",
            "  │HTTP 500                                                          │",
        ]
    );
    // The selection (moved down once) is highlighted, the current model is
    // bold, headers are cyan and the failed endpoint's warning is yellow.
    assert!(buffer[(3, 5)].modifier.contains(Modifier::REVERSED));
    assert!(!buffer[(3, 4)].modifier.contains(Modifier::REVERSED));
    assert!(buffer[(3, 4)].modifier.contains(Modifier::BOLD));
    assert_eq!(buffer[(3, 3)].fg, Color::Cyan);
    assert_eq!(buffer[(3, 8)].fg, Color::Yellow);
}

#[test]
fn selected_cells_render_reversed() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let mouse = |kind, column| {
        Msg::Mouse(MouseEvent {
            kind,
            column,
            row: 0,
            modifiers: KeyModifiers::NONE,
        })
    };
    let mut app = sample_app();
    // The first draw records which logical line each row shows.
    render_buffer(&mut app, W, H);
    app.update(mouse(MouseEventKind::Down(MouseButton::Left), 2));
    app.update(mouse(MouseEventKind::Drag(MouseButton::Left), 5));
    let buffer = render_buffer(&mut app, W, H);
    assert_eq!(rows(&buffer)[0], "› list files");
    let reversed: Vec<u16> = (0..W)
        .filter(|&x| buffer[(x, 0)].modifier.contains(Modifier::REVERSED))
        .collect();
    assert_eq!(reversed, vec![2, 3, 4, 5]);
    // Other transcript rows are untouched.
    assert!((0..W).all(|x| !buffer[(x, 2)].modifier.contains(Modifier::REVERSED)));

    app.update(mouse(MouseEventKind::Up(MouseButton::Left), 5));
    assert_eq!(app.take_clipboard_text().as_deref(), Some("list"));
}

/// `event` of the helper `label`, as the helper follower forwards it.
fn helper(label: &str, event: AgentEvent) -> Msg {
    use meerkat_core::{ScopedAgentEvent, StreamScopeFrame};
    Msg::Scoped(ScopedAgentEvent::new(
        vec![
            StreamScopeFrame::Primary {
                session_id: "s".into(),
            },
            StreamScopeFrame::MobMember {
                flow_run_id: String::new(),
                agent_identity: label.into(),
                agent_runtime_id: None,
                fence_token: None,
                generation: None,
            },
        ],
        event,
    ))
}

/// A transcript with a prompt and the helper `scout` (two calls).
fn helper_app() -> App {
    let mut app = App::new(StatusInfo::default());
    app.transcript.note_helper_model("scout", "opus");
    app.transcript.push(Block::User {
        text: "find it".into(),
    });
    for (id, name, args) in [
        ("a", "glob", json!({"pattern": "*.rs"})),
        ("b", "grep", json!({"pattern": "fn resolve"})),
    ] {
        app.update(helper(
            "scout",
            AgentEvent::ToolCallRequested {
                id: id.into(),
                name: name.into(),
                args: ToolCallArguments::from_value(args).unwrap(),
            },
        ));
    }
    app
}

#[test]
fn a_folded_helper_renders_one_line() {
    let mut app = helper_app();
    let rows = render_sized(&mut app, Instant::now(), 70);
    assert_eq!(rows[0], "› find it");
    assert_eq!(rows[1], "");
    assert_eq!(
        rows[2],
        "▸ [scout · opus] ⠋ running · 2 tool calls · grep \"fn resolve\""
    );
    // Nothing of the helper's calls is drawn.
    assert!(rows[3..8].iter().all(String::is_empty), "{rows:?}");
    assert!(rows[11].ends_with("ctrl+↓ helpers"), "{:?}", rows[11]);
}

#[test]
fn the_helper_panel_renders_under_the_input() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = helper_app();
    let rows = render_sized(&mut app, Instant::now(), 70);
    // Transcript (rows 0-7), the input's border and line, one panel line,
    // the status bar.
    assert!(rows[8].starts_with('─'), "{:?}", rows[8]);
    assert_eq!(rows[9], "›");
    assert_eq!(
        rows[10],
        "  [scout · opus] ⠋ running · 2 tool calls · grep \"fn resolve\""
    );
    // Ctrl+Down selects it: drawn reversed, and the hint says what Enter
    // does.
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Down,
        KeyModifiers::CONTROL,
    )));
    let mut terminal = Terminal::new(TestBackend::new(70, H)).unwrap();
    terminal
        .draw(|f| draw_at(f, &mut app, Instant::now()))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let selected = self::rows(&buffer);
    assert!(buffer[(4, 10)].modifier.contains(Modifier::REVERSED));
    assert!(!buffer[(4, 2)].modifier.contains(Modifier::REVERSED));
    assert!(
        selected[8].contains("Enter opens the helper"),
        "{:?}",
        selected[8]
    );
}

#[test]
fn an_expanded_helper_renders_its_blocks() {
    let mut app = helper_app();
    let header = app.transcript.helper_header("scout").unwrap();
    assert!(app.transcript.set_helper_expanded(header, true));
    let rows = render_sized(&mut app, Instant::now(), 70);
    assert_eq!(
        rows[2],
        "▾ [scout · opus] ⠋ running · 2 tool calls · grep \"fn resolve\""
    );
    assert_eq!(rows[4], "  ○ [scout · opus] glob({\"pattern\":\"*.rs\"})");
    assert_eq!(
        rows[6],
        "  ○ [scout · opus] grep({\"pattern\":\"fn resolve\"})"
    );
}

#[test]
fn a_helper_full_view_renders_title_prose_reasoning_and_tools() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = App::new(StatusInfo::default());
    app.transcript.note_helper_model("scout", "opus");
    app.transcript.push(Block::User {
        text: "find it".into(),
    });
    for event in [
        AgentEvent::ReasoningDelta {
            delta: "Where would it be?".into(),
            assistant_message_id: None,
        },
        AgentEvent::TextDelta {
            delta: "Looking in **src**.".into(),
            assistant_message_id: None,
        },
        AgentEvent::ToolCallRequested {
            id: "a".into(),
            name: "glob".into(),
            args: ToolCallArguments::from_value(json!({"pattern": "*.rs"})).unwrap(),
        },
    ] {
        app.update(helper("scout", event));
    }
    app.update(Msg::Key(KeyEvent::new(
        KeyCode::Down,
        KeyModifiers::CONTROL,
    )));
    app.update(Msg::Key(KeyEvent::from(KeyCode::Enter)));
    let mut terminal = Terminal::new(TestBackend::new(70, H)).unwrap();
    terminal
        .draw(|f| draw_at(f, &mut app, Instant::now()))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let rows = rows(&buffer);
    assert_eq!(
        rows[0],
        "◂ [scout · opus] ⠋ running · 1 tool call · glob \"*.rs\"  Esc back"
    );
    assert!(buffer[(0, 0)].modifier.contains(Modifier::REVERSED));
    assert_eq!(rows[1], "✻ thinking (18 chars, Ctrl-O to expand)");
    assert_eq!(rows[2], "");
    assert_eq!(rows[3], "• Looking in src.");
    assert_eq!(rows[4], "");
    assert_eq!(rows[5], "  ○ [scout · opus] glob({\"pattern\":\"*.rs\"})");
    // The transcript's own blocks are not drawn.
    assert!(rows.iter().all(|r| !r.contains("find it")), "{rows:?}");
    assert!(
        rows[8].contains("Esc back to the transcript"),
        "{:?}",
        rows[8]
    );
}
