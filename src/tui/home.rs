//! Database-first landing page. Only unlocked summary metadata is shown.
use super::{App, KeyCode, KeyEvent, Kind};
use crate::i18n::Language;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

impl App {
    fn home_action_row(&self) -> Option<(String, String, bool)> {
        if self.home_tree_focus {
            let tree = self.library.as_ref()?.category_tree();
            let (category, _) = tree.get(self.home_tree_selected.checked_sub(1)?)?;
            Some((category.id.clone(), category.title.clone(), true))
        } else {
            self.home_rows().get(self.home_selected).cloned()
        }
    }
    pub(super) fn home_rows(&self) -> Vec<(String, String, bool)> {
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
        if self.database_picker {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.database_picker = false,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.database_selected =
                        (self.database_selected + 1).min(self.databases.len().saturating_sub(1))
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.database_selected = self.database_selected.saturating_sub(1)
                }
                KeyCode::Enter => {
                    if let Some(database) = self.databases.get(self.database_selected) {
                        if database.current {
                            self.database_picker = false;
                            self.show_form(Kind::Library);
                        } else {
                            self.show_form(Kind::SwitchDatabase {
                                id: database.id.clone(),
                                name: database.name.clone(),
                            });
                        }
                    }
                }
                KeyCode::Char('o') => {
                    self.database_picker = false;
                    self.show_form(Kind::OpenLocal);
                }
                KeyCode::Char('n') => {
                    self.database_picker = false;
                    self.show_form(Kind::Init);
                }
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::Char('d') {
            match crate::databases::list(&self.store) {
                Ok(databases) => {
                    self.databases = databases;
                    self.database_selected = 0;
                    self.database_picker = true;
                }
                Err(error) => self.error(error),
            }
            return;
        }
        if key.code == KeyCode::Tab || key.code == KeyCode::BackTab {
            self.home_tree_focus = !self.home_tree_focus;
            return;
        }
        if self.home_tree_focus
            && let Some(library) = &self.library
        {
            let tree = library.category_tree();
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.home_tree_selected = (self.home_tree_selected + 1).min(tree.len());
                    return;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.home_tree_selected = self.home_tree_selected.saturating_sub(1);
                    return;
                }
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                    self.category = self
                        .home_tree_selected
                        .checked_sub(1)
                        .and_then(|i| tree.get(i))
                        .map(|(c, _)| c.id.clone());
                    self.home_selected = 0;
                    self.home_tree_focus = false;
                    return;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Char('e') if self.library.is_some() => {
                if let Some((id, title, folder)) = self.home_action_row() {
                    if folder {
                        self.show_form(Kind::RenameCategory { id, title });
                    } else if let Some(name) = self.config.as_ref().and_then(|config| {
                        config
                            .connections
                            .iter()
                            .find(|(_, c)| c.credential_id == id)
                            .map(|(name, _)| name.clone())
                    }) {
                        self.show_form(Kind::Token(name));
                    }
                }
            }
            KeyCode::Char('c') if self.config.is_some() => {
                if self.home_tree_focus {
                    self.category = self.home_action_row().map(|(id, _, _)| id);
                    self.home_selected = 0;
                }
                self.show_form(Kind::Connect);
            }
            KeyCode::Char('n') if self.library.is_some() => {
                let parent = if self.home_tree_focus {
                    self.home_action_row().map(|(id, _, _)| id)
                } else {
                    self.category.clone()
                };
                self.show_form(Kind::Category(parent))
            }
            KeyCode::Char('m') if self.library.is_some() => {
                if let Some((id, _, _)) = self.home_action_row() {
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
    if app.database_picker {
        let area =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(2)]).split(frame.area());
        let rows: Vec<_> = app
            .databases
            .iter()
            .map(|db| {
                ListItem::new(format!(
                    "{} {}\n  {}",
                    if db.current { "●" } else { "○" },
                    super::view::clean(&db.name),
                    super::view::clean(&db.path.display().to_string())
                ))
            })
            .collect();
        let mut state =
            ListState::default().with_selected((!rows.is_empty()).then_some(app.database_selected));
        frame.render_stateful_widget(
            List::new(rows)
                .block(Block::default().borders(Borders::ALL).title(if en {
                    " Databases "
                } else {
                    " 我的数据库 "
                }))
                .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
                .highlight_symbol("› "),
            area[0],
            &mut state,
        );
        frame.render_widget(
            Paragraph::new(if en {
                "Enter Open   n New   o Add MDBX file   Esc Back"
            } else {
                "Enter 打开   n 新建   o 添加 MDBX 文件   Esc 返回"
            }),
            area[1],
        );
        return;
    }
    let root = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .split(frame.area());
    let name = app
        .config
        .as_ref()
        .and_then(|c| {
            c.database_name
                .as_deref()
                .or_else(|| c.vault.file_stem().and_then(|s| s.to_str()))
        })
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
    let mut navigation = vec![ListItem::new(format!("  {name}"))];
    if let Some(library) = &app.library {
        for (category, depth) in library.category_tree() {
            navigation.push(ListItem::new(format!(
                " {}{}",
                "  ".repeat(depth.min(10)),
                super::view::clean(&category.title)
            )));
        }
    }
    app.home_tree_selected = app
        .home_tree_selected
        .min(navigation.len().saturating_sub(1));
    let mut tree_state = ListState::default().with_selected(Some(app.home_tree_selected));
    frame.render_stateful_widget(
        List::new(navigation)
            .block(Block::default().borders(Borders::RIGHT).title(if en {
                " Databases / Tab "
            } else {
                " 数据库与分类 / Tab "
            }))
            .highlight_style(if app.home_tree_focus {
                Style::default().bg(Color::DarkGray).fg(Color::White)
            } else {
                Style::default().fg(Color::Cyan)
            })
            .highlight_symbol("› "),
        columns[0],
        &mut tree_state,
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
        " Enter Open  h Parent  n Category  c Token  m Move\n Tab Panes  e Edit  d Databases  o Add  L Lock  F3 Settings  q Quit".to_owned()
    } else {
        " Enter 打开  h 上一级  n 分类  c Token  m 移动\n Tab 切换栏  e 编辑  d 数据库  o 添加  L 锁定  F3 设置  q 退出"
            .to_owned()
    };
    frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: false }), root[2]);
}
