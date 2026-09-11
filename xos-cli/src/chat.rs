//! `xos chat` — the default XOS interface.
//!
//! Instant, no licence constraints, and it runs on hardware where a browser
//! struggles. It also validates the daemon end to end with no web configuration
//! in the way.
//!
//! It holds no model logic. Every completion is a JSON-RPC `complete` call to
//! xosd, and the transcript this file keeps is only what gets sent back as
//! context. Nothing here talks to a model.

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde_json::{json, Value};

use crate::socket::{Connection, StreamEvent};
use crate::theme;

/// What the worker thread reports back to the interface.
enum Update {
    Delta(String),
    Done(Value),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
    System,
}

struct Entry {
    role: Role,
    text: String,
}

/// Everything the daemon told us about where completions will run.
struct ProviderInfo {
    name: String,
    local: bool,
}

struct Chat {
    entries: Vec<Entry>,
    input: String,
    provider: ProviderInfo,
    socket: PathBuf,
    halted: bool,
    /// Set while a reply is streaming.
    generating: Option<Generation>,
    last_rate: Option<f64>,
    updates: Option<Receiver<Update>>,
    scroll: u16,
    follow: bool,
}

struct Generation {
    started: Instant,
    first_token: Option<Instant>,
    /// The daemon emits one delta per token, so counting deltas tracks the
    /// generation rate live. The exact count arrives with the final usage.
    tokens: u32,
}

impl Generation {
    fn rate(&self) -> Option<f64> {
        let first = self.first_token?;
        let seconds = first.elapsed().as_secs_f64();
        if seconds <= 0.05 || self.tokens == 0 {
            return None;
        }
        Some(self.tokens as f64 / seconds)
    }
}

pub fn run(socket_override: Option<PathBuf>) -> Result<(), String> {
    // Learn where completions will run before drawing anything, so the status
    // line is truthful from the first frame.
    let mut connection = Connection::open(socket_override.clone())?;
    let status = connection.call("status", json!({}))?;
    let socket = connection.path().to_path_buf();
    drop(connection);

    let default_provider = status
        .get("default_provider")
        .and_then(Value::as_str)
        .unwrap_or("none")
        .to_string();
    let halted = status.get("halted").and_then(Value::as_bool).unwrap_or(false);
    let local = status
        .get("providers")
        .and_then(Value::as_array)
        .and_then(|providers| {
            providers
                .iter()
                .find(|p| p.get("name").and_then(Value::as_str) == Some(&default_provider))
        })
        .and_then(|p| p.pointer("/capabilities/local"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let mut chat = Chat {
        entries: Vec::new(),
        input: String::new(),
        provider: ProviderInfo {
            name: default_provider,
            local,
        },
        socket,
        halted,
        generating: None,
        last_rate: None,
        updates: None,
        scroll: 0,
        follow: true,
    };
    if halted {
        chat.entries.push(Entry {
            role: Role::System,
            text: "XOS is halted. Run `xos resume` in another terminal to start it again."
                .to_string(),
        });
    }

    let mut terminal = enter()?;
    let outcome = event_loop(&mut terminal, &mut chat, socket_override);
    leave(&mut terminal);
    outcome
}

fn enter() -> Result<Terminal<CrosstermBackend<Stdout>>, String> {
    enable_raw_mode().map_err(|e| format!("cannot take the terminal: {}", e))?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| e.to_string())?;

    // A panic must not leave the terminal in raw mode.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        previous(info);
    }));

    Terminal::new(CrosstermBackend::new(stdout)).map_err(|e| e.to_string())
}

fn leave(terminal: &mut Terminal<CrosstermBackend<Stdout>>) {
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    chat: &mut Chat,
    socket_override: Option<PathBuf>,
) -> Result<(), String> {
    loop {
        terminal
            .draw(|frame| draw(frame, chat))
            .map_err(|e| e.to_string())?;

        drain(chat);

        // Short poll so streaming stays smooth without spinning the CPU.
        if event::poll(Duration::from_millis(40)).map_err(|e| e.to_string())? {
            match event::read().map_err(|e| e.to_string())? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if handle_key(key, chat, &socket_override) {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
    }
}

/// Returns true when the user asked to leave.
fn handle_key(key: KeyEvent, chat: &mut Chat, socket_override: &Option<PathBuf>) -> bool {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('c') if control => return true,
        KeyCode::Char('l') if control => {
            chat.entries.clear();
            chat.scroll = 0;
            chat.follow = true;
        }
        KeyCode::Enter => {
            let text = chat.input.trim().to_string();
            if !text.is_empty() && chat.generating.is_none() {
                chat.input.clear();
                send(chat, text, socket_override);
            }
        }
        KeyCode::Backspace => {
            chat.input.pop();
        }
        KeyCode::Up => {
            chat.scroll = chat.scroll.saturating_sub(1);
            chat.follow = false;
        }
        KeyCode::Down => {
            chat.scroll = chat.scroll.saturating_add(1);
        }
        KeyCode::PageUp => {
            chat.scroll = chat.scroll.saturating_sub(10);
            chat.follow = false;
        }
        KeyCode::PageDown => {
            chat.scroll = chat.scroll.saturating_add(10);
        }
        KeyCode::Char(c) => chat.input.push(c),
        _ => {}
    }
    false
}

/// Hand the conversation to xosd and stream the reply back.
fn send(chat: &mut Chat, text: String, socket_override: &Option<PathBuf>) {
    chat.entries.push(Entry {
        role: Role::User,
        text,
    });
    chat.entries.push(Entry {
        role: Role::Assistant,
        text: String::new(),
    });
    chat.follow = true;

    let messages: Vec<Value> = chat
        .entries
        .iter()
        .filter(|entry| entry.role != Role::System)
        .filter(|entry| !entry.text.is_empty())
        .map(|entry| {
            json!({
                "role": match entry.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::System => "system",
                },
                "content": entry.text
            })
        })
        .collect();

    let (sender, receiver): (Sender<Update>, Receiver<Update>) = mpsc::channel();
    chat.updates = Some(receiver);
    chat.generating = Some(Generation {
        started: Instant::now(),
        first_token: None,
        tokens: 0,
    });

    let socket_override = socket_override.clone();
    thread::spawn(move || {
        // A connection of its own, so a long completion never blocks another
        // client of the daemon.
        let mut connection = match Connection::open(socket_override) {
            Ok(connection) => connection,
            Err(error) => {
                let _ = sender.send(Update::Failed(error));
                return;
            }
        };
        let outcome = connection.stream("complete", json!({"messages": messages}), |event| {
            let update = match event {
                StreamEvent::Delta(text) => Update::Delta(text),
                StreamEvent::Done(result) => Update::Done(result),
                StreamEvent::Failed(error) => Update::Failed(error),
            };
            let _ = sender.send(update);
        });
        if let Err(error) = outcome {
            let _ = sender.send(Update::Failed(error));
        }
    });
}

/// Fold whatever the worker has produced into the transcript.
fn drain(chat: &mut Chat) {
    let Some(receiver) = &chat.updates else {
        return;
    };
    let mut finished = false;
    let mut failure = None;

    while let Ok(update) = receiver.try_recv() {
        match update {
            Update::Delta(text) => {
                if let Some(generation) = &mut chat.generating {
                    if generation.first_token.is_none() {
                        generation.first_token = Some(Instant::now());
                    }
                    generation.tokens += 1;
                }
                if let Some(entry) = chat.entries.last_mut() {
                    entry.text.push_str(&text);
                }
            }
            Update::Done(result) => {
                // The exact count is authoritative over the live estimate.
                if let Some(generation) = &chat.generating {
                    let tokens = result
                        .pointer("/usage/output_tokens")
                        .and_then(Value::as_u64)
                        .map(|t| t as f64)
                        .unwrap_or(generation.tokens as f64);
                    let seconds = generation
                        .first_token
                        .unwrap_or(generation.started)
                        .elapsed()
                        .as_secs_f64();
                    if seconds > 0.0 && tokens > 0.0 {
                        chat.last_rate = Some(tokens / seconds);
                    }
                }
                finished = true;
            }
            Update::Failed(error) => {
                failure = Some(error);
                finished = true;
            }
        }
    }

    if let Some(error) = failure {
        // Drop the empty assistant turn; an error is not a reply.
        if chat
            .entries
            .last()
            .map(|e| e.role == Role::Assistant && e.text.is_empty())
            .unwrap_or(false)
        {
            chat.entries.pop();
        }
        chat.entries.push(Entry {
            role: Role::System,
            text: error,
        });
    }
    if finished {
        chat.generating = None;
        chat.updates = None;
    }
}

fn draw(frame: &mut Frame, chat: &mut Chat) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::BASE)),
        area,
    );

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_transcript(frame, chat, rows[0]);
    draw_input(frame, chat, rows[1]);
    draw_status(frame, chat, rows[2]);
}

fn draw_transcript(frame: &mut Frame, chat: &mut Chat, area: Rect) {
    let tier = theme::tier(chat.provider.local);
    let mut lines: Vec<Line> = Vec::new();

    for entry in &chat.entries {
        let (label, colour) = match entry.role {
            Role::User => ("you", theme::TEXT_DIM),
            Role::Assistant => ("xos", tier),
            Role::System => ("!", theme::STOP),
        };
        lines.push(Line::from(vec![Span::styled(
            format!("{:<4}", label),
            Style::default().fg(colour).add_modifier(Modifier::BOLD),
        )]));
        for piece in entry.text.split('\n') {
            lines.push(Line::from(vec![
                Span::raw("     "),
                Span::styled(
                    piece.to_string(),
                    Style::default().fg(match entry.role {
                        Role::System => theme::STOP,
                        _ => theme::TEXT,
                    }),
                ),
            ]));
        }
        lines.push(Line::from(""));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "Ask XOS something. Enter sends, Ctrl+L clears, Ctrl+C leaves.",
            Style::default().fg(theme::TEXT_FAINT),
        )));
    }

    // Follow the tail while generating, unless the reader has scrolled away.
    let height = area.height.saturating_sub(2);
    let total = lines.len() as u16;
    if chat.follow {
        chat.scroll = total.saturating_sub(height);
    } else {
        chat.scroll = chat.scroll.min(total.saturating_sub(1));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::LINE))
        .style(Style::default().bg(theme::BASE));

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((chat.scroll, 0)),
        area,
    );
}

fn draw_input(frame: &mut Frame, chat: &Chat, area: Rect) {
    let busy = chat.generating.is_some();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme::LINE))
        .style(Style::default().bg(theme::SURFACE));

    let body = if busy {
        Span::styled(
            "waiting for the reply",
            Style::default().fg(theme::TEXT_FAINT),
        )
    } else {
        Span::styled(
            format!("{}\u{2588}", chat.input),
            Style::default().fg(theme::TEXT),
        )
    };

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(theme::TEXT_DIM)),
            body,
        ]))
        .block(block)
        .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_status(frame: &mut Frame, chat: &Chat, area: Rect) {
    let tier = theme::tier(chat.provider.local);
    let mut spans = vec![
        Span::styled(
            format!(" {} ", chat.provider.name),
            Style::default().fg(tier).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if chat.provider.local { "local" } else { "api" },
            Style::default().fg(tier),
        ),
        Span::styled("  \u{00b7}  ", Style::default().fg(theme::TEXT_FAINT)),
    ];

    if chat.halted {
        spans.push(Span::styled("halted", Style::default().fg(theme::STOP)));
        spans.push(Span::styled("  \u{00b7}  ", Style::default().fg(theme::TEXT_FAINT)));
    }

    match chat.generating.as_ref().and_then(Generation::rate) {
        Some(rate) => spans.push(Span::styled(
            format!("{:.1} tok/s", rate),
            Style::default().fg(theme::TEXT),
        )),
        None => {
            if chat.generating.is_some() {
                spans.push(Span::styled(
                    "generating",
                    Style::default().fg(theme::TEXT_DIM),
                ));
            } else if let Some(rate) = chat.last_rate {
                spans.push(Span::styled(
                    format!("{:.1} tok/s last", rate),
                    Style::default().fg(theme::TEXT_DIM),
                ));
            } else {
                spans.push(Span::styled("idle", Style::default().fg(theme::TEXT_DIM)));
            }
        }
    }

    spans.push(Span::styled(
        format!("   {}", chat.socket.display()),
        Style::default().fg(theme::TEXT_FAINT),
    ));

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::SURFACE)),
        area,
    );
}
