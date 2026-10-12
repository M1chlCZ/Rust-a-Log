use crate::live::{self, Event, Shared, Source};
use crossterm::event::{
    self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, Wrap};
use std::io;
use std::time::Duration;

#[derive(Default)]
struct View {
    source: usize,
    selected: usize,
    paused: bool,
    search: String,
    editing: bool,
    level: usize,
    details: bool,
    detail_scroll: u16,
    horizontal: usize,
}

const LEVELS: [&str; 7] = [
    "ALL", "ERROR", "WARNING", "INFO", "DEBUG", "TRACE", "SUCCESS",
];

impl View {
    fn pan(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.horizontal = self.horizontal.saturating_sub(20);
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.horizontal = self.horizontal.saturating_add(20);
            }
            KeyCode::Home => self.horizontal = 0,
            _ => return false,
        }
        true
    }
    fn clamp_selection(&mut self, events: &[Event], sources: &[Source]) {
        self.selected = self
            .selected
            .min(self.visible(events, sources).len().saturating_sub(1));
    }
    fn visible<'a>(&self, events: &'a [Event], sources: &[Source]) -> Vec<&'a Event> {
        let source = self
            .source
            .checked_sub(1)
            .and_then(|index| sources.get(index))
            .map(|source| source.id);
        let text = self.search.to_lowercase();
        events
            .iter()
            .filter(|event| {
                source.is_none_or(|id| event.session == id)
                    && (self.level == 0 || event.record.level == LEVELS[self.level])
                    && (text.is_empty()
                        || event.record.message.to_lowercase().contains(&text)
                        || event
                            .record
                            .fields
                            .to_string()
                            .to_lowercase()
                            .contains(&text))
            })
            .collect()
    }
}

fn color(level: &str) -> Color {
    match level {
        "ERROR" => Color::Red,
        "WARNING" => Color::Yellow,
        "DEBUG" => Color::Cyan,
        "TRACE" => Color::Magenta,
        "SUCCESS" => Color::Green,
        _ => Color::White,
    }
}

fn log_row(event: &Event, show_source: bool) -> String {
    let timestamp = chrono::DateTime::parse_from_rfc3339(&event.record.timestamp)
        .map(|time| time.format("%H:%M:%S").to_string())
        .unwrap_or_else(|_| event.record.timestamp.clone());
    let mut row = format!("{} [{}] ", timestamp, event.record.level);
    if show_source {
        row.push_str(&format!("{}: ", event.source));
    }
    row.push_str(&event.record.message);
    if let Some(fields) = event.record.fields.as_object() {
        let priority = ["method", "route", "path", "status", "duration_ms", "error"];
        for key in priority.iter().copied().chain(
            fields
                .keys()
                .map(String::as_str)
                .filter(|key| !priority.contains(key)),
        ) {
            if let Some(value) = fields.get(key) {
                row.push_str(&format!(" {key}={value}"));
            }
        }
    }
    live::safe_text(&row)
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

fn draw(
    frame: &mut ratatui::Frame,
    view: &View,
    events: &[Event],
    sources: &[Source],
    status: &str,
) {
    let areas = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(frame.area());
    frame.render_widget(Paragraph::new(status), areas[0]);
    let columns = Layout::horizontal([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(areas[1]);
    let mut items = vec![ListItem::new("All sources")];
    items.extend(sources.iter().map(|source| {
        ListItem::new(format!(
            "{} #{} {}\n{} · {} logs\n{} dropped",
            if source.connected { "live" } else { "offline" },
            source.id,
            live::safe_text(&source.name),
            live::safe_text(&source.language),
            source.records,
            source.dropped
        ))
    }));
    let mut source_state = ListState::default().with_selected(Some(view.source));
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(" Sources "))
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan)),
        columns[0],
        &mut source_state,
    );
    let visible = view.visible(events, sources);
    let record_areas = if view.details {
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(columns[1])
            .to_vec()
    } else {
        vec![columns[1]]
    };
    let logs: Vec<_> = visible
        .iter()
        .map(|event| {
            let row = log_row(event, view.source == 0 && sources.len() > 1)
                .chars()
                .skip(view.horizontal)
                .take(usize::from(record_areas[0].width.saturating_sub(2)))
                .collect::<String>();
            ListItem::new(row).style(Style::default().fg(color(&event.record.level)))
        })
        .collect();
    let mut log_state = ListState::default()
        .with_selected(Some(view.selected.min(visible.len().saturating_sub(1))));
    frame.render_stateful_widget(
        List::new(logs)
            .block(Block::bordered().title(if view.paused {
                " Logs [paused snapshot] "
            } else {
                " Logs [live] "
            }))
            .highlight_style(Style::default().bg(Color::DarkGray)),
        record_areas[0],
        &mut log_state,
    );
    if view.details {
        let detail = visible
            .get(view.selected.min(visible.len().saturating_sub(1)))
            .map(|event| {
                format!(
                    "{} [{}] {}\n\n{}\n\n{}",
                    live::safe_text(&event.record.timestamp),
                    event.record.level,
                    live::safe_text(&event.source),
                    live::safe_text(&event.record.message),
                    serde_json::to_string_pretty(&event.record.fields).unwrap_or_default()
                )
            })
            .unwrap_or_default();
        frame.render_widget(
            Paragraph::new(live::safe_text(&detail))
                .wrap(Wrap { trim: false })
                .scroll((view.detail_scroll, 0))
                .block(Block::bordered().title(" Record details ")),
            record_areas[1],
        );
    }
    let help = format!(
        "←/→ source  ↑/↓ record  Space pause  Enter details\nShift+←/→ pan  Home reset  PgUp/Dn details  l level  / find  q quit\n{}{} | level: {} | pan: {}",
        if view.editing { "Search: " } else { "Filter: " },
        live::safe_text(&view.search),
        LEVELS[view.level],
        view.horizontal
    );
    frame.render_widget(Paragraph::new(help), areas[2]);
}

pub fn run(state: &Shared, address: &str) -> io::Result<()> {
    let mut view = View::default();
    let (mut events, mut sources) = (Vec::new(), Vec::new());
    ratatui::run(|terminal| {
        loop {
            let status = {
                let state = state.lock().unwrap();
                sources.clone_from(&state.sources.iter().cloned().collect());
                view.source = view.source.min(sources.len());
                if !view.paused {
                    events = state.events.iter().cloned().collect();
                    view.selected = view.visible(&events, &sources).len().saturating_sub(1);
                }
                format!(
                    "rual · {address} · {} records / {} KiB · {} expired · {} rejected\n{}",
                    state.events.len(),
                    state.bytes / 1024,
                    state.evicted,
                    state.rejected,
                    live::safe_text(&state.last_error)
                )
            };
            terminal.draw(|frame| draw(frame, &view, &events, &sources, &status))?;
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            let TerminalEvent::Key(key) = event::read()? else {
                continue;
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                break;
            }
            if view.editing {
                match key.code {
                    KeyCode::Enter | KeyCode::Esc => view.editing = false,
                    KeyCode::Backspace => {
                        view.search.pop();
                    }
                    KeyCode::Char(c) if view.search.len() < 256 => view.search.push(c),
                    _ => {}
                }
                view.selected = view.visible(&events, &sources).len().saturating_sub(1);
                continue;
            }
            if view.pan(key) {
                continue;
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('/') => view.editing = true,
                KeyCode::Char('l') => view.level = (view.level + 1) % LEVELS.len(),
                KeyCode::Char(' ') => view.paused = !view.paused,
                KeyCode::Left => view.source = view.source.saturating_sub(1),
                KeyCode::Right => view.source = (view.source + 1).min(sources.len()),
                KeyCode::Up => {
                    view.paused = true;
                    view.selected = view.selected.saturating_sub(1);
                }
                KeyCode::Down => {
                    view.paused = true;
                    view.selected = (view.selected + 1)
                        .min(view.visible(&events, &sources).len().saturating_sub(1));
                }
                KeyCode::Enter => {
                    view.details = !view.details;
                    view.detail_scroll = 0;
                }
                KeyCode::PageUp => view.detail_scroll = view.detail_scroll.saturating_sub(10),
                KeyCode::PageDown => view.detail_scroll = view.detail_scroll.saturating_add(10),
                _ => {}
            }
            view.clamp_selection(&events, &sources);
            if !matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
                view.detail_scroll = 0;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn request_fixture(message: &str) -> (Vec<Event>, Vec<Source>) {
        let mut state = live::State::default();
        let id = state
            .register("doglio-backend-lan".into(), "go".into())
            .unwrap();
        state.push(
            id,
            live::Record {
                timestamp: "2026-10-12T16:09:46.193369077Z".into(),
                level: "INFO".into(),
                message: message.into(),
                fields: serde_json::json!({
                    "request_id": "test-42", "method": "GET", "route": "/admin/orders",
                    "status": 200, "duration_ms": 12, "context": {"attempt": 1}
                }),
            },
        );
        (
            state.events.into_iter().collect(),
            state.sources.into_iter().collect(),
        )
    }

    fn render(view: &View, events: &[Event], sources: &[Source], width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 18)).unwrap();
        terminal
            .draw(|frame| draw(frame, view, events, sources, "rual"))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn request_fields_are_visible_without_opening_details_or_repeating_source() {
        let (events, sources) = request_fixture("http_request");
        for source in [0, 1] {
            let screen = render(
                &View {
                    source,
                    ..View::default()
                },
                &events,
                &sources,
                300,
            );
            assert!(screen.contains("16:09:46 [INFO] http_request method=\"GET\" route=\"/admin/orders\" status=200 duration_ms=12"), "{screen}");
            assert!(screen.contains("request_id=\"test-42\""));
            assert!(screen.contains("context={\"attempt\":1}"));
            assert_eq!(screen.matches("doglio-backend-lan").count(), 1);
            assert!(!screen.contains("2026-10-12T"));
        }
    }

    #[test]
    fn long_multiline_messages_are_complete_safe_single_line_rows() {
        let message = format!("{}\nnext\tcolumn\x1b[2J", "a".repeat(500));
        let (events, sources) = request_fixture(&message);
        let screen = render(&View::default(), &events, &sources, 1600);
        assert!(screen.contains(&format!("{}\\nnext\\tcolumn�[2J", "a".repeat(500))));
        assert!(!screen.contains('\x1b'));
    }

    #[test]
    fn detail_preserves_the_full_timestamp_and_source_context() {
        let (events, sources) = request_fixture("http_request");
        let screen = render(
            &View {
                details: true,
                ..View::default()
            },
            &events,
            &sources,
            180,
        );
        let details = screen.split("Record details").nth(1).unwrap();
        assert!(details.contains("2026-10-12T16:09:46.193369077Z"));
        assert!(details.contains("doglio-backend-lan"));
    }

    #[test]
    fn horizontal_pan_reveals_the_tail_and_does_not_change_source() {
        let (events, sources) = request_fixture(&format!("{} tail-visible", "a".repeat(500)));
        let mut view = View {
            source: 1,
            ..View::default()
        };
        let initial = render(&view, &events, &sources, 100);
        assert!(!initial.contains("tail-visible"));
        for _ in 0..25 {
            assert!(view.pan(KeyEvent::new(KeyCode::Right, KeyModifiers::SHIFT)));
        }
        assert_eq!(view.source, 1);
        assert!(render(&view, &events, &sources, 100).contains("tail-visible"));
        assert!(view.pan(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE)));
        assert_eq!(render(&view, &events, &sources, 100), initial);
        assert!(!view.pan(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE)));
        assert!(view.pan(KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT)));
        assert_eq!(view.horizontal, 0);
    }

    #[test]
    fn source_names_only_repeat_when_all_sources_need_disambiguation() {
        let (events, mut sources) = request_fixture("http_request");
        let mut second = sources[0].clone();
        second.id = 2;
        second.name = "worker".into();
        sources.push(second);
        assert_eq!(
            render(&View::default(), &events, &sources, 300)
                .matches("doglio-backend-lan")
                .count(),
            2
        );
        assert_eq!(
            render(
                &View {
                    source: 1,
                    ..View::default()
                },
                &events,
                &sources,
                300
            )
            .matches("doglio-backend-lan")
            .count(),
            1
        );
    }

    #[test]
    fn paused_source_and_level_filters_clamp_the_record_selection() {
        let mut state = live::State::default();
        let first = state.register("first".into(), "go".into()).unwrap();
        let second = state.register("second".into(), "node".into()).unwrap();
        for _ in 0..20 {
            state.push(first, live::Record::from_line("[INFO] many"));
        }
        for _ in 0..3 {
            state.push(second, live::Record::from_line("[ERROR] few"));
        }
        let events: Vec<_> = state.events.iter().cloned().collect();
        let sources: Vec<_> = state.sources.iter().cloned().collect();
        let mut view = View {
            paused: true,
            selected: 22,
            source: 2,
            ..View::default()
        };
        view.clamp_selection(&events, &sources);
        assert_eq!(view.selected, 2);
        view.level = 3;
        view.clamp_selection(&events, &sources);
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn sources_filters_details_and_control_bytes_render_safely() {
        let mut state = live::State::default();
        let id = state.register("worker".into(), "go".into()).unwrap();
        state.push(
            id,
            live::Record::from_line("[ERROR] hello\x1b[2J\nstack trace"),
        );
        state.disconnect(id);
        let events: Vec<_> = state.events.iter().cloned().collect();
        let sources: Vec<_> = state.sources.iter().cloned().collect();
        let view = View {
            details: true,
            search: "hello".into(),
            level: 1,
            ..View::default()
        };
        assert_eq!(view.visible(&events, &sources).len(), 1);
        let mut terminal = Terminal::new(TestBackend::new(120, 25)).unwrap();
        terminal
            .draw(|frame| draw(frame, &view, &events, &sources, "rual"))
            .unwrap();
        let contents: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            contents.contains("worker")
                && contents.contains("offline")
                && contents.contains("stack trace")
        );
        assert!(!contents.contains('\x1b'));
    }
}
