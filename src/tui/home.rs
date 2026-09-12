//! Database-first landing page. Only unlocked summary metadata is shown.
use super::{App, KeyCode, KeyEvent, Kind};
use crate::i18n::Language;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

impl App {
    fn home_rows(&self) -> Vec<(String, String, bool)> {
        let Some(library) = &self.library else {
            return vec![];
        };
        let mut rows: Vec<_> = library
            .categories
            .iter()
            .filter(|c| match self.category.as_deref() {
                Some(id) => c.parent.as_deref() == Some(id),
                None => c
                    .parent
                    .as_ref()
                    .is_none_or(|id| !library.categories.iter().any(|p| &p.id == id)),
            })
            .map(|c| (c.id.clone(), c.title.clone(), true))
            .collect();
        rows.extend(
            library
                .entries
                .iter()
                .filter(|e| self.category.as_deref() == Some(e.category.as_str()))
                .map(|e| (e.id.clone(), e.title.clone(), false)),
        );
        rows
    }

    pub(super) fn home_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('c') if self.config.is_some() => self.show_form(Kind::Connect),
            KeyCode::Char('n') if self.library.is_some() => {
                self.show_form(Kind::Category(self.category.clone()))
            }
            KeyCode::Char('m') if self.library.is_some() => {
                if let Some((id, _, _)) = self.home_rows().get(self.home_selected) {
                    self.show_form(Kind::Move(id.clone()));
                }
            }
            KeyCode::Char('q') => self.quitting = true,
            KeyCode::Char(',') => self.home = false,
            KeyCode::Char('u') | KeyCode::Enter if self.library.is_none() => {
                if self.config.is_some() {
                    self.show_form(Kind::Library);
                } else {
                    self.show_form(Kind::Init);
                }
            }
            KeyCode::Char('o') => self.show_form(Kind::OpenLocal),
            KeyCode::Char('L') => {
                self.library = None;
                self.library_loaded = None;
                self.invoke("lock");
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.home_selected = self
                    .home_selected
                    .saturating_add(1)
                    .min(self.home_rows().len().saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.home_selected = self.home_selected.saturating_sub(1)
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some((id, _, true)) = self.home_rows().get(self.home_selected) {
                    self.category = Some(id.clone());
                    self.home_selected = 0;
                }
            }
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => {
                self.category = self
                    .library
                    .as_ref()
                    .and_then(|library| {
                        library
                            .categories
                            .iter()
                            .find(|c| Some(&c.id) == self.category.as_ref())
                    })
                    .and_then(|c| c.parent.clone());
                self.home_selected = 0;
            }
            _ => {}
        }
    }
}

pub(super) fn render(frame: &mut Frame<'_>, app: &mut App) {
    let en = app.language == Language::En;
    let root = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .split(frame.area());
    let name = app
        .config
        .as_ref()
        .and_then(|c| c.vault.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("Monica");
    let category = app
        .library
        .as_ref()
        .and_then(|l| {
            l.categories
                .iter()
                .find(|c| Some(&c.id) == app.category.as_ref())
        })
        .map(|c| c.title.as_str());
    frame.render_widget(
        Paragraph::new(format!(
            "  Monica  /  {name}{}",
            category
                .map(|c| format!(" / {}", super::view::clean(c)))
                .unwrap_or_default()
        ))
        .style(Style::default().fg(Color::Cyan).bold()),
        root[0],
    );
    let columns = Layout::horizontal([
        Constraint::Percentage(24),
        Constraint::Percentage(42),
        Constraint::Percentage(34),
    ])
    .split(root[1]);
    let mut navigation = vec![Line::raw(format!("  {name}")), Line::raw("")];
    if let Some(library) = &app.library {
        for c in library
            .categories
            .iter()
            .take(columns[0].height.saturating_sub(5) as usize)
        {
            let mut depth = 0;
            let mut parent = c.parent.as_deref();
            while let Some(id) = parent {
                if depth >= 8 {
                    break;
                }
                depth += 1;
                parent = library
                    .categories
                    .iter()
                    .find(|p| p.id == id)
                    .and_then(|p| p.parent.as_deref());
            }
            navigation.push(Line::raw(format!(
                " {}{}",
                "  ".repeat(depth),
                super::view::clean(&c.title)
            )));
        }
    }
    frame.render_widget(
        Paragraph::new(navigation).block(Block::default().borders(Borders::RIGHT).title(if en {
            " Databases "
        } else {
            " 数据库与分类 "
        })),
        columns[0],
    );
    if app.library.is_none() {
        let hint = if app.config.is_none() {
            if en {
                "Create or open a database\n\nEnter  Create database\no      Open MDBX file"
            } else {
                "添加你的第一个数据库\n\nEnter  新建数据库\no      打开 MDBX 文件"
            }
        } else if en {
            "Database locked\n\nEnter  Unlock and browse\no      Open another database"
        } else {
            "数据库已锁定\n\nEnter  解锁并查看条目\no      打开其他数据库"
        };
        frame.render_widget(
            Paragraph::new(hint)
                .block(Block::default().borders(Borders::ALL).title(if en {
                    " Library "
                } else {
                    " 我的条目 "
                }))
                .wrap(Wrap { trim: false }),
            columns[1],
        );
    } else {
        let rows = app.home_rows();
        app.home_selected = app.home_selected.min(rows.len().saturating_sub(1));
        let items: Vec<_> = rows
            .iter()
            .map(|(_, title, folder)| {
                ListItem::new(format!(
                    "{} {}",
                    if *folder { "+" } else { "·" },
                    super::view::clean(title)
                ))
            })
            .collect();
        let mut state =
            ListState::default().with_selected((!rows.is_empty()).then_some(app.home_selected));
        frame.render_stateful_widget(
            List::new(items)
                .block(Block::default().borders(Borders::ALL).title(if en {
                    " Entries "
                } else {
                    " 分类与条目 "
                }))
                .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
                .highlight_symbol("› "),
            columns[1],
            &mut state,
        );
        if let Some((id, title, folder)) = rows.get(app.home_selected) {
            let mut detail = format!(
                "{}\n\n{}",
                super::view::clean(title),
                if *folder {
                    if en {
                        "Category\nEnter to open"
                    } else {
                        "分类\n按 Enter 打开"
                    }
                } else if en {
                    "Encrypted entry"
                } else {
                    "加密条目"
                }
            );
            if let Some(entry) = app
                .library
                .as_ref()
                .and_then(|l| l.entries.iter().find(|e| &e.id == id))
            {
                detail.push_str(&format!("\n\n{}", entry.kind));
                if entry.kind == "api-token" {
                    detail.push_str(if en {
                        "\n\nToken  ••••••••"
                    } else {
                        "\n\nToken  ••••••••（已加密）"
                    });
                }
            }
            frame.render_widget(
                Paragraph::new(detail)
                    .block(Block::default().borders(Borders::LEFT).title(if en {
                        " Details "
                    } else {
                        " 详情 "
                    }))
                    .wrap(Wrap { trim: false }),
                columns[2],
            );
        }
    }
    let footer = if app.failed {
        app.message.clone()
    } else if app.pending.is_some() {
        if en {
            "Opening database…"
        } else {
            "正在打开数据库…"
        }
        .to_owned()
    } else if en {
        " Enter Open  h Parent  n Category  c Token  m Move\n o Database  L Lock  F3 Settings  q Quit".to_owned()
    } else {
        " Enter 打开  h 上一级  n 分类  c Token  m 移动\n o 数据库  L 锁定  F3 设置  q 退出"
            .to_owned()
    };
    frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: false }), root[2]);
}
