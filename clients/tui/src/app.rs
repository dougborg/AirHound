use crate::backend::Snapshot;
use ratatui::{
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use serde_json::Value;

fn safe_text(value: &str) -> String {
    value.chars().filter(|ch| !ch.is_control()).collect()
}

#[derive(Default)]
pub struct App {
    pub snapshot: Option<Snapshot>,
    pub selection: ListState,
    pub busy: bool,
    pub error: Option<String>,
    pub stale: bool,
    pub validation: Option<Value>,
    pub show_validation: bool,
    pub show_status: bool,
    pub scroll: u16,
}
impl App {
    pub fn devices(&self) -> &[Value] {
        self.snapshot
            .as_ref()
            .and_then(|s| s.inventory["devices"].as_array())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
    pub fn selected(&self) -> Option<&Value> {
        self.selection
            .selected()
            .and_then(|index| self.devices().get(index))
    }
    pub fn begin(&mut self) {
        self.busy = true;
        self.error = None;
    }
    pub fn refreshed(&mut self, result: Result<Snapshot, String>) {
        self.busy = false;
        match result {
            Ok(snapshot) => {
                let selected_id = self
                    .selected()
                    .and_then(|d| d["id"].as_str())
                    .map(str::to_owned);
                self.snapshot = Some(snapshot);
                let index = selected_id
                    .and_then(|id| self.devices().iter().position(|d| d["id"] == id))
                    .or_else(|| (!self.devices().is_empty()).then_some(0));
                self.selection.select(index);
                self.stale = false;
                self.scroll = 0;
            }
            Err(error) => {
                self.error = Some(error);
                self.stale = true;
            }
        }
    }
    pub fn validated(&mut self, result: Result<Value, String>) {
        self.busy = false;
        self.show_validation = true;
        self.show_status = false;
        self.scroll = 0;
        match result {
            Ok(value) => {
                self.validation = Some(value);
            }
            Err(error) => {
                self.validation = None;
                self.error = Some(error);
            }
        }
    }
    pub fn navigate(&mut self, down: bool) {
        let count = self.devices().len();
        if count == 0 {
            self.selection.select(None);
            return;
        }
        let current = self.selection.selected().unwrap_or(0);
        self.selection.select(Some(if down {
            (current + 1).min(count - 1)
        } else {
            current.saturating_sub(1)
        }));
        self.scroll = 0;
        self.show_validation = false;
        self.show_status = false;
    }
    pub fn draw(&mut self, frame: &mut Frame, session: Option<&str>) {
        let area = frame.area();
        if area.width < 60 || area.height < 12 {
            frame.render_widget(
                Paragraph::new(
                    "AirHound\nTerminal needs 60 columns × 12 rows.\nq Quit · r Refresh",
                )
                .wrap(Wrap { trim: false }),
                area,
            );
            return;
        }
        let regions = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(area);
        let connection = match &self.snapshot {
            Some(snapshot) => format!(
                "{} · {} · {}{}",
                safe_text(&snapshot.build),
                safe_text(&snapshot.mode),
                safe_text(&snapshot.instance),
                if self.stale { " · STALE" } else { "" }
            ),
            None => "Not connected".into(),
        };
        let status = if self.busy {
            "Request in progress; q cancels this client"
        } else {
            "Passive client · no capture controls"
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "AirHound",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(connection),
                Line::from(status),
            ]),
            regions[0],
        );
        let columns = Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(regions[1]);
        let items: Vec<ListItem> = self
            .devices()
            .iter()
            .map(|device| {
                let name = device["name"].as_str().unwrap_or("Unnamed device");
                let states = device["candidates"]
                    .as_array()
                    .map(|candidates| {
                        candidates
                            .iter()
                            .filter_map(|c| c["state"].as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                ListItem::new(vec![
                    Line::from(safe_text(name)),
                    Line::from(safe_text(&states.replace('_', " "))),
                ])
            })
            .collect();
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .title(format!(" Receivers ({}) ", self.devices().len())),
            )
            .highlight_symbol("> ")
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(list, columns[0], &mut self.selection);
        let (title, value) = if self.show_status {
            (
                " Service status · backend response ",
                self.snapshot.as_ref().map(|snapshot| &snapshot.status),
            )
        } else if self.show_validation {
            (" Validation · no devices opened ", self.validation.as_ref())
        } else {
            (" Receiver details · readiness unverified ", self.selected())
        };
        let mut detail = value.map(|v| serde_json::to_string_pretty(v).unwrap_or_default()).unwrap_or_else(|| if self.show_validation { "Press v to validate the --session file through the backend.".into() } else if self.snapshot.is_some() {"No supported receivers reported. Press r to refresh.".into()} else {"Connect to an existing service with --socket PATH. This client never starts the service or a radio.".into()});
        if !self.show_validation && !self.show_status {
            if let Some(snapshot) = &self.snapshot {
                if let Some(diagnostics) = snapshot.inventory["diagnostics"].as_array() {
                    for diagnostic in diagnostics {
                        detail.push_str(&format!(
                            "\n\nDiscovery: {}",
                            safe_text(
                                diagnostic["message"]
                                    .as_str()
                                    .unwrap_or("No diagnostic message")
                            )
                        ));
                    }
                }
            }
        }
        frame.render_widget(
            Paragraph::new(detail)
                .wrap(Wrap { trim: false })
                .scroll((self.scroll, 0))
                .block(Block::default().borders(Borders::TOP).title(title)),
            columns[1],
        );
        let notice = self.error.as_deref().unwrap_or_else(|| {
            if session.is_some() {
                "Validation does not reserve devices or prove permission/RF readiness."
            } else {
                "Add --session PATH to enable backend validation."
            }
        });
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(
                    "q Quit   r Refresh   ↑/↓ Select   Tab View   PgUp/PgDn Scroll   v Validate",
                ),
                Line::from(safe_text(notice)),
            ])
            .wrap(Wrap { trim: false }),
            regions[2],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};
    fn snapshot(ids: &[&str]) -> Snapshot {
        Snapshot {
            instance: "fixture-service".into(),
            build: "test".into(),
            mode: "passive_control".into(),
            status: serde_json::json!({}),
            inventory: serde_json::json!({"devices":ids.iter().map(|id|serde_json::json!({"id":id,"name":format!("Receiver {id}"),"candidates":[{"state":"unverified"}]})).collect::<Vec<_>>(),"diagnostics":[{"message":"Permission not probed\u{001b}"}]}),
        }
    }
    #[test]
    fn selection_survives_reordering_and_failed_refresh_is_stale() {
        let mut app = App::default();
        app.refreshed(Ok(snapshot(&["a", "b"])));
        app.navigate(true);
        app.refreshed(Ok(snapshot(&["b", "a"])));
        assert_eq!(app.selected().unwrap()["id"], "b");
        app.refreshed(Err("disconnected".into()));
        assert!(app.stale);
        assert_eq!(app.devices().len(), 2);
        app.refreshed(Ok(snapshot(&[])));
        assert!(app.selected().is_none());
        app.navigate(true);
    }
    #[test]
    fn terminal_render_keeps_status_uncertainty_and_errors_visible() {
        let mut app = App::default();
        app.refreshed(Ok(snapshot(&["a"])));
        app.refreshed(Err("Service unavailable".into()));
        let mut terminal = Terminal::new(TestBackend::new(110, 24)).unwrap();
        terminal
            .draw(|frame| app.draw(frame, Some("session.json")))
            .unwrap();
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!text.contains('\x1b'));
        for expected in [
            "STALE",
            "Receiver a",
            "unverified",
            "Permission not probed",
            "Service unavailable",
            "no capture controls",
        ] {
            assert!(text.contains(expected), "{expected}");
        }
        let mut tiny = Terminal::new(TestBackend::new(30, 8)).unwrap();
        tiny.draw(|frame| app.draw(frame, None)).unwrap();
        assert!(tiny
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
            .contains("AirHound"));
    }
}
