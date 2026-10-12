use crate::live::{self, Event, Shared, Source};
use crossterm::event::{self, Event as TerminalEvent, KeyCode, KeyEventKind, KeyModifiers};
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
}

const LEVELS: [&str; 7] = [
    "ALL", "ERROR", "WARNING", "INFO", "DEBUG", "TRACE", "SUCCESS",
];

impl View {
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
            let message = live::safe_text(&event.record.message)
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(300)
                .collect::<String>();
            ListItem::new(format!(
                "{} [{}] {}: {}",
                live::safe_text(&event.record.timestamp),
                event.record.level,
                live::safe_text(&event.source),
                message
            ))
            .style(Style::default().fg(color(&event.record.level)))
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
                    "{}\n\n{}",
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
        "←/→ source  ↑/↓ record  Space pause  Enter details\nPgUp/PgDn scroll  l level  / search  q quit\n{}{} | level: {}",
        if view.editing { "Search: " } else { "Filter: " },
        live::safe_text(&view.search),
        LEVELS[view.level]
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
