//! Explicit human disclosure, isolated from public previews and machine output.
use super::view::{BG, FG};
use super::{Action, App, KeyCode, KeyEvent, KeyModifiers};
use crate::i18n::Language;
use crate::object::Inspection;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Style,
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use zeroize::Zeroizing;

pub(super) struct Viewer {
    object: Inspection,
    selected: usize,
    revealed: bool,
    scroll: usize,
    lines: Vec<Zeroizing<String>>,
    width: u16,
    page: usize,
}

impl Viewer {
    pub(super) fn new(object: Inspection) -> Self {
        Self {
            object,
            selected: 0,
            revealed: false,
            scroll: 0,
            lines: Vec::new(),
            width: 0,
            page: 1,
        }
    }

    pub(super) fn expired(&self) -> bool {
        self.object.opened.elapsed() >= std::time::Duration::from_secs(60)
    }

    fn mask(&mut self) {
        self.revealed = false;
        self.lines.clear();
        self.scroll = 0;
        self.width = 0;
    }

    pub(super) fn render(&mut self, frame: &mut Frame<'_>, lang: Language) {
        let area = frame.area();
        frame.render_widget(Block::default().style(Style::default().fg(FG).bg(BG)), area);
        let sections = Layout::vertical([
            Constraint::Length(6),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);
        let (name, value) = &self.object.fields[self.selected];
        let summary = &self.object.summary;
        let status = if self.object.editable {
            "adapter available"
        } else {
            "read-only / 只读"
        };
        let title = escape(&String::from_utf8_lossy(
            summary.title.as_deref().unwrap_or_default(),
        ));
        let metadata = format!(
            "{}\n{} · payload v{} · {status}\nID: {}\nCollection: {}\n[{}/{}] {}",
            title.as_str(),
            summary.object_type_id,
            summary.payload_schema_version,
            summary.object_id,
            summary.collection_id,
            self.selected + 1,
            self.object.fields.len(),
            escape(name).as_str()
        );
        frame.render_widget(Paragraph::new(metadata), sections[0]);
        let panel = Block::default().borders(Borders::ALL);
        let body = panel.inner(sections[1]);
        frame.render_widget(panel, sections[1]);
        self.page = body.height.max(1) as usize;
        if self.revealed {
            if self.width != body.width {
                self.lines = wrap(&escape(value), body.width);
                self.width = body.width;
            }
            self.scroll = self.scroll.min(self.lines.len().saturating_sub(self.page));
            let lines: Vec<_> = self
                .lines
                .iter()
                .skip(self.scroll)
                .take(self.page)
                .map(|text| Line::raw(text.as_str()))
                .collect();
            frame.render_widget(Paragraph::new(lines), body);
        } else {
            frame.render_widget(
                Paragraph::new(if lang == Language::En {
                    "••••••  Press Space to reveal this field"
                } else {
                    "••••••  按空格显示当前字段"
                }),
                body,
            );
        }
        frame.render_widget(Paragraph::new(if lang == Language::En {
            "←/→ field · Space show/hide · ↑/↓ PgUp/PgDn Home/End scroll\nEsc close · L lock · Clears after 60s or focus loss"
        } else { "←/→ 切换字段 · 空格显示/隐藏 · ↑/↓ PgUp/PgDn Home/End 滚动\nEsc 关闭 · L 锁定 · 60 秒或失焦后清除" }), sections[2]);
    }
}

// Terminal control and bidi formatting characters are represented visibly,
// never executed or silently removed. Actual payload bytes remain untouched.
fn escape(text: &str) -> Zeroizing<String> {
    let mut escaped = Zeroizing::new(String::new());
    for ch in text.chars() {
        if (ch.is_control() && ch != '\n')
            || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            escaped.extend(ch.escape_unicode());
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

fn wrap(text: &str, width: u16) -> Vec<Zeroizing<String>> {
    let mut lines = Vec::new();
    let mut line = Zeroizing::new(String::new());
    let mut used = 0;
    for ch in text.chars() {
        let size = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if ch == '\n' || used + size > usize::from(width.max(2)) {
            lines.push(line);
            line = Zeroizing::new(String::new());
            used = 0;
        }
        if ch != '\n' {
            line.push(ch);
            used += size;
        }
    }
    lines.push(line);
    lines
}

impl App {
    pub(super) fn inspection_key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.inspection = None;
            self.quitting = true;
            return;
        }
        let Some(viewer) = &mut self.inspection else {
            return;
        };
        if viewer.expired() {
            self.inspection = None;
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Tab | KeyCode::BackTab => {
                self.inspection = None
            }
            KeyCode::Char('L') => self.start(Action::Lock),
            KeyCode::Char(' ') => {
                if viewer.revealed {
                    viewer.mask();
                } else {
                    viewer.revealed = true;
                }
            }
            KeyCode::Right | KeyCode::Left => {
                viewer.mask();
                viewer.selected = if key.code == KeyCode::Right {
                    (viewer.selected + 1).min(viewer.object.fields.len() - 1)
                } else {
                    viewer.selected.saturating_sub(1)
                };
            }
            KeyCode::Down | KeyCode::Char('j') => viewer.scroll = viewer.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => viewer.scroll = viewer.scroll.saturating_sub(1),
            KeyCode::PageDown => viewer.scroll = viewer.scroll.saturating_add(viewer.page),
            KeyCode::PageUp => viewer.scroll = viewer.scroll.saturating_sub(viewer.page),
            KeyCode::Home => viewer.scroll = 0,
            KeyCode::End => viewer.scroll = usize::MAX,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdbx_core::{model::ObjectTypeId, tiga::TigaMode};
    use mdbx_storage::repo::{CommitContext, EntryRepo};

    #[test]
    fn viewer_masks_each_field_preserves_long_values_and_expires() {
        let directory = tempfile::tempdir().unwrap();
        let vault = crate::vault::Vault::create(
            &directory.path().join("view.mdbx"),
            "synthetic-password",
            TigaMode::Multi,
        )
        .unwrap();
        let collection = vault.create_category("test", None).unwrap();
        let entry = EntryRepo::create(&vault.runtime.read().unwrap(), &CommitContext::new("test".into()),
            &collection, "com.example.future".parse::<ObjectTypeId>().unwrap(), Some("Future"),
            &serde_json::json!({"first":"synthetic-hidden-value", "second": format!("{}END-OF-VALUE", "中".repeat(70000))})).unwrap();
        let mut app = App::new(
            crate::config::ConfigStore::new(directory.path().join("gateway.json")),
            Language::En,
        );
        app.inspection = Some(Viewer::new(vault.inspect_object(&entry.entry_id).unwrap()));
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 24)).unwrap();
        let render = |terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
                      app: &mut App| {
            terminal
                .draw(|frame| super::super::view::render(frame, app))
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        assert!(!render(&mut terminal, &mut app).contains("synthetic-hidden-value"));
        app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(render(&mut terminal, &mut app).contains("synthetic-hidden-value"));
        app.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(!render(&mut terminal, &mut app).contains("END-OF-VALUE"));
        app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        render(&mut terminal, &mut app);
        app.key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert!(render(&mut terminal, &mut app).contains("END-OF-VALUE"));
        app.inspection.as_mut().unwrap().object.opened -= std::time::Duration::from_secs(61);
        app.key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(app.inspection.is_none());
        assert_eq!(escape("a\u{1b}\u{202e}b").as_str(), "a\\u{1b}\\u{202e}b");
    }
}
