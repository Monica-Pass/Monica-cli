//! Database-first landing page. The left rail lists databases, the middle column
//! is one continuous category/entry tree, and the right column previews the
//! selected row. Only unlocked summary metadata is drawn; nothing here opens the
//! vault or resolves a credential.
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use zeroize::Zeroizing;

use super::browser::{Filter, Focus};
use super::form::{Input, Kind};
use super::fuzzy;
use super::preview::{field, heading, label, scope_title};
use super::view::{
    ACCENT, BG, CYAN, DIM, ERROR, FG, GREEN, Icon, LIGHT, Record, WARNING, broker_status,
    capsule_edge, chrome, clean, clipped, draw_record, path_line, position, rails, render_input,
    wrapped_lines,
};
use super::{App, KeyCode, KeyEvent, Mode, Page};
use crate::i18n::{Language, Message};
use crate::keys::payload::LOGIN_TYPE_SSH;
use crate::library::Library;
use crate::model::Provider;
use crate::tr;
use crate::vault::KeyEntrySummary;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HomeAction {
    Open,
    OpenLocal,
    NewDatabase,
    Authorizations,
}

/// One selectable row of the middle column. A category is the section header of
/// the rows below it, so the two kinds never compete in one flat list.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum HomeRow {
    Category {
        id: String,
        title: String,
        path: String,
        depth: usize,
        entries: usize,
        subs: usize,
    },
    Entry {
        id: String,
        title: String,
        kind: String,
        category: String,
        path: String,
        depth: usize,
    },
    Action {
        action: HomeAction,
        title: Message,
    },
}

impl HomeRow {
    pub(super) fn id(&self) -> &str {
        match self {
            Self::Category { id, .. } | Self::Entry { id, .. } => id,
            Self::Action { action, .. } => match action {
                HomeAction::Open => "action:browse",
                HomeAction::OpenLocal => "action:open",
                HomeAction::NewDatabase => "action:new",
                HomeAction::Authorizations => "action:grants",
            },
        }
    }

    fn depth(&self) -> usize {
        match self {
            Self::Category { depth, .. } | Self::Entry { depth, .. } => *depth,
            Self::Action { .. } => 0,
        }
    }

    fn kind(&self) -> Message {
        match self {
            Self::Category { .. } => Message::KindCategory,
            Self::Entry { .. } => Message::KindEntry,
            Self::Action { .. } => Message::KindAction,
        }
    }
}

fn path_of(library: &Library, id: &str) -> String {
    let mut titles = Vec::new();
    let mut current = Some(id.to_owned());
    for _ in 0..=library.categories.len() {
        let found = current
            .as_deref()
            .and_then(|id| library.categories.iter().find(|c| c.id == id));
        let Some(category) = found else { break };
        titles.push(category.title.clone());
        current = category.parent.clone();
    }
    titles.reverse();
    titles.join(" / ")
}

/// English agrees a count with its noun; Chinese uses one form for both.
fn counted(lang: Language, count: usize, one: Message, many: Message) -> String {
    format!("{count} {}", lang.text(if count == 1 { one } else { many }))
}

fn walk(library: &Library, parent: Option<&str>, depth: usize, rows: &mut Vec<HomeRow>) {
    for category in library
        .categories
        .iter()
        .filter(|c| c.parent.as_deref() == parent)
    {
        rows.push(HomeRow::Category {
            id: category.id.clone(),
            title: category.title.clone(),
            path: String::new(),
            depth,
            entries: library
                .entries
                .iter()
                .filter(|e| e.category == category.id)
                .count(),
            subs: library
                .categories
                .iter()
                .filter(|c| c.parent.as_deref() == Some(category.id.as_str()))
                .count(),
        });
        walk(library, Some(category.id.as_str()), depth + 1, rows);
    }
    for entry in library
        .entries
        .iter()
        .filter(|e| Some(e.category.as_str()) == parent)
    {
        rows.push(HomeRow::Entry {
            id: entry.id.clone(),
            title: entry.title.clone(),
            kind: entry.kind.clone(),
            category: entry.category.clone(),
            path: String::new(),
            depth,
        });
    }
}

impl App {
    pub(super) fn home_rows(&self) -> Vec<HomeRow> {
        let Some(library) = &self.library else {
            return self.home_actions();
        };
        let query = self.home_filter.trim().to_lowercase();
        if !query.is_empty() {
            return self.home_matches(library, &query);
        }
        let mut rows = Vec::new();
        walk(library, self.category.as_deref(), 0, &mut rows);
        if self.category.is_some() {
            return rows;
        }
        // Orphaned rows stay reachable instead of silently vanishing.
        for entry in library
            .entries
            .iter()
            .filter(|e| !library.categories.iter().any(|c| c.id == e.category))
        {
            rows.push(HomeRow::Entry {
                id: entry.id.clone(),
                title: entry.title.clone(),
                kind: entry.kind.clone(),
                category: entry.category.clone(),
                path: String::new(),
                depth: 0,
            });
        }
        if let Some(row) = self.grants_row() {
            rows.insert(0, row);
        }
        rows
    }

    /// Named route to the authorization list, kept first at the tree root so the
    /// live token proxy is one Enter away without opening the detailed database.
    fn grants_row(&self) -> Option<HomeRow> {
        self.config.as_ref().map(|_| HomeRow::Action {
            action: HomeAction::Authorizations,
            title: Message::PageGrants,
        })
    }

    fn home_actions(&self) -> Vec<HomeRow> {
        let mut rows = Vec::new();
        if self.config.is_some() {
            rows.push(HomeRow::Action {
                action: HomeAction::Open,
                title: Message::OpenDatabaseRow,
            });
        } else {
            rows.push(HomeRow::Action {
                action: HomeAction::NewDatabase,
                title: Message::NewDatabaseRow,
            });
        }
        rows.push(HomeRow::Action {
            action: HomeAction::OpenLocal,
            title: Message::OpenLocalRow,
        });
        // Reachable while the vault is locked: grant state needs no master password.
        rows.extend(self.grants_row());
        rows
    }

    fn home_matches(&self, library: &Library, query: &str) -> Vec<HomeRow> {
        let hits = |haystack: String| fuzzy::score(&haystack, query);
        let owned = |id: &str| {
            library
                .categories
                .iter()
                .find(|c| c.id == id)
                .map_or_else(String::new, |c| path_of(library, &c.id))
        };
        let mut scored: Vec<_> = library
            .categories
            .iter()
            .filter_map(|c| {
                let score = hits(format!("{} {}", owned(&c.id), c.title))?;
                Some((
                    score,
                    HomeRow::Category {
                        id: c.id.clone(),
                        title: c.title.clone(),
                        path: owned(&c.id),
                        depth: 0,
                        entries: library
                            .entries
                            .iter()
                            .filter(|e| e.category == c.id)
                            .count(),
                        subs: library
                            .categories
                            .iter()
                            .filter(|sub| sub.parent.as_deref() == Some(c.id.as_str()))
                            .count(),
                    },
                ))
            })
            .collect();
        scored.extend(library.entries.iter().filter_map(|e| {
            let path = owned(&e.category);
            let score = match self.keys.iter().find(|key| key.entry_id == e.id) {
                Some(key) => hits(format!(
                    "{path} {} {} {} {}",
                    e.title,
                    if key.login_type == LOGIN_TYPE_SSH {
                        "ssh"
                    } else {
                        "gpg"
                    },
                    key_label(key, self.language),
                    key.fingerprint
                ))?,
                None => hits(format!("{path} {} {}", e.title, e.kind))?,
            };
            Some((
                score,
                HomeRow::Entry {
                    id: e.id.clone(),
                    title: e.title.clone(),
                    kind: e.kind.clone(),
                    category: e.category.clone(),
                    path,
                    depth: 0,
                },
            ))
        }));
        // Best match first, tree order within one score. Cached so the tie-break key is
        // built once per row; a plain comparator would rebuild it on every comparison.
        scored.sort_by_cached_key(|(score, row)| (std::cmp::Reverse(*score), sort_key(row)));
        let mut rows: Vec<_> = scored.into_iter().map(|(_, row)| row).collect();
        if let Some(row) = self.grants_row() {
            let title = self.language.text(Message::PageGrants);
            if fuzzy::score(title, query).is_some() {
                rows.insert(0, row);
            }
        }
        rows
    }

    pub(super) fn selected_home_row(&self) -> Option<HomeRow> {
        self.home_rows().into_iter().nth(self.home_selected)
    }

    pub(super) fn home_database_index(&self) -> Option<usize> {
        self.databases
            .len()
            .checked_sub(1)
            .map(|last| self.home_rail_selected.min(last))
    }

    pub(super) fn begin_home_filter(&mut self) {
        self.focus = Focus::List;
        self.mode = Mode::Filter(Filter {
            input: Input::new(&self.home_filter, 256),
            previous: Zeroizing::new(self.home_filter.to_string()),
            selection: self.selected_home_row().map(|row| row.id().to_owned()),
        });
    }

    pub(super) fn update_home_filter(&mut self, text: &str) {
        let key = self.selected_home_row().map(|row| row.id().to_owned());
        self.home_filter = Zeroizing::new(text.to_owned());
        self.home_selected = self
            .home_rows()
            .iter()
            .position(|row| Some(row.id()) == key.as_deref())
            .unwrap_or(0);
        self.home_offset = 0;
    }

    pub(super) fn restore_home_selection(&mut self, id: Option<&str>) {
        self.home_selected = self
            .home_rows()
            .iter()
            .position(|row| Some(row.id()) == id)
            .unwrap_or(0);
        self.home_offset = 0;
    }

    fn run_home_action(&mut self, action: HomeAction) {
        match action {
            HomeAction::Open => self.show_form(Kind::Library),
            HomeAction::OpenLocal => self.show_form(Kind::OpenLocal),
            HomeAction::NewDatabase => self.show_form(Kind::Init),
            HomeAction::Authorizations => self.set_page(Page::Grants),
        }
    }

    pub(super) fn home_key(&mut self, key: KeyEvent) {
        if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            self.shift_focus(key.code == KeyCode::Tab);
            return;
        }
        match key.code {
            KeyCode::Char(',') | KeyCode::F(3) => self.home = false,
            KeyCode::Char('?') | KeyCode::F(1) => self.show_help(),
            KeyCode::Char('!') => self.show_message(),
            KeyCode::Char('d') => self.focus = Focus::Navigation,
            KeyCode::Char(':') => self.mode = Mode::Command(Input::new("", 32)),
            KeyCode::Char('/') if self.library.is_some() => self.begin_home_filter(),
            KeyCode::Esc => {
                self.home_filter = Zeroizing::new(String::new());
                self.home_selected = 0;
                self.home_offset = 0;
                self.focus = Focus::List;
                self.preview_scroll = 0;
                self.message_at = None;
            }
            KeyCode::Char('q') => self.quitting = true,
            _ => self.home_pane_key(key),
        }
    }

    fn home_pane_key(&mut self, key: KeyEvent) {
        let half = (self.page_size / 2).max(1);
        if self.focus == Focus::Navigation {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.home_rail_selected =
                        (self.home_rail_selected + 1).min(self.databases.len().saturating_sub(1))
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.home_rail_selected = self.home_rail_selected.saturating_sub(1)
                }
                KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
                    let current = self
                        .home_database_index()
                        .and_then(|index| self.databases.get(index))
                        .is_some_and(|database| database.current);
                    if current && self.library.is_some() {
                        self.focus = Focus::List;
                        return;
                    }
                    self.select_home_database();
                }
                KeyCode::Char('o') => self.show_form(Kind::OpenLocal),
                KeyCode::Char('n') => self.show_form(Kind::Init),
                _ => {}
            }
            return;
        }
        if self.focus == Focus::Preview {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.preview_scroll = (self.preview_scroll + 1).min(self.preview_max_scroll)
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.preview_scroll = self.preview_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => {
                    self.preview_scroll = (self.preview_scroll + half).min(self.preview_max_scroll)
                }
                KeyCode::PageUp => self.preview_scroll = self.preview_scroll.saturating_sub(half),
                KeyCode::Char('h')
                | KeyCode::Char('l')
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::Enter => self.focus = Focus::List,
                _ => {}
            }
            return;
        }
        let rows = self.home_rows().len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.home_selected = self
                    .home_selected
                    .saturating_add(1)
                    .min(rows.saturating_sub(1))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.home_selected = self.home_selected.saturating_sub(1)
            }
            KeyCode::PageDown => {
                self.home_selected = self
                    .home_selected
                    .saturating_add(half)
                    .min(rows.saturating_sub(1))
            }
            KeyCode::PageUp => self.home_selected = self.home_selected.saturating_sub(half),
            KeyCode::Char('g') | KeyCode::Home => self.home_selected = 0,
            KeyCode::Char('G') | KeyCode::End => self.home_selected = rows.saturating_sub(1),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                match self.selected_home_row() {
                    Some(HomeRow::Category { id, .. }) => {
                        self.category = Some(id);
                        self.home_selected = 0;
                    }
                    Some(HomeRow::Action { action, .. }) => self.run_home_action(action),
                    Some(_) => self.focus = Focus::Preview,
                    None => {}
                }
            }
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => {
                if self.category.is_none() {
                    self.focus = Focus::Navigation;
                    return;
                }
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
            KeyCode::Char('o') => self.show_form(Kind::OpenLocal),
            KeyCode::Char('n') if self.library.is_none() => {
                self.run_home_action(HomeAction::NewDatabase)
            }
            KeyCode::Char('n') => {
                let parent = match self.selected_home_row() {
                    Some(HomeRow::Category { id, .. }) => Some(id),
                    _ => self.category.clone(),
                };
                self.show_form(Kind::Category(parent))
            }
            KeyCode::Char('c') if self.library.is_some() => self.show_form(Kind::Connect),
            KeyCode::Char('S') => self.invoke("ssh"),
            KeyCode::Char('I') => self.invoke("sshimport"),
            KeyCode::Char('A') => self.invoke("gpg"),
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.copy_public_field(key.code == KeyCode::Char('Y'))
            }
            KeyCode::Char('m') | KeyCode::Char('e') if self.library.is_some() => {
                let Some(row) = self.selected_home_row() else {
                    return;
                };
                if key.code == KeyCode::Char('m') {
                    self.show_form(Kind::Move(row.id().to_owned()));
                    return;
                }
                match &row {
                    HomeRow::Category { id, title, .. } => {
                        self.show_form(Kind::RenameCategory {
                            id: id.clone(),
                            title: title.clone(),
                        });
                    }
                    HomeRow::Entry { .. } => {
                        if let Some(entry) = self.key_row(&row) {
                            let form = Kind::EditKey {
                                entry_id: entry.entry_id.clone(),
                                login_type: entry.login_type.clone(),
                                title: entry.title.clone(),
                                comment: entry.comment.clone(),
                                notes: entry.notes.clone(),
                            };
                            self.show_form(form);
                            return;
                        }
                        if let Some((name, _)) = self.home_entry_connection(&row) {
                            self.show_form(Kind::Token(name));
                        }
                    }
                    HomeRow::Action { action, .. } => self.run_home_action(*action),
                }
            }
            KeyCode::Char('r') if self.library.is_some() => {
                let Some(row) = self.selected_home_row() else {
                    return;
                };
                if let HomeRow::Entry { title, .. } = &row
                    && let Some((name, _)) = self.home_entry_connection(&row)
                {
                    self.show_form(Kind::RenameEntry {
                        name,
                        title: title.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    fn select_home_database(&mut self) {
        let Some(index) = self.home_database_index() else {
            return;
        };
        let Some(database) = self.databases.get(index).cloned() else {
            return;
        };
        if database.current {
            self.show_form(Kind::Library);
        } else {
            self.show_form(Kind::SwitchDatabase {
                id: database.id.clone(),
                name: database.name.clone(),
            });
        }
    }

    pub(super) fn home_entry_connection(&self, row: &HomeRow) -> Option<(String, Provider)> {
        let HomeRow::Entry { id, .. } = row else {
            return None;
        };
        let config = self.config.as_ref()?;
        config
            .connections
            .iter()
            .find(|(_, connection)| connection.credential_id == *id)
            .map(|(name, connection)| (name.clone(), connection.provider))
    }

    /// The key entry behind a tree row, when that row is one. A `login` row carries no
    /// `login_type` in its plaintext summary, so only the unlock that read the tree can
    /// tell a key apart from an ordinary credential.
    pub(super) fn key_row(&self, row: &HomeRow) -> Option<&KeyEntrySummary> {
        let HomeRow::Entry { id, .. } = row else {
            return None;
        };
        self.keys.iter().find(|key| key.entry_id == *id)
    }

    /// `y` puts a key row's public line on the clipboard, `Y` its fingerprint. Every value
    /// read here already came out of the unlocked summary, so private material and gateway
    /// tokens have no path into the clipboard, and no row can ask for one.
    pub(super) fn copy_public_field(&mut self, fingerprint: bool) {
        let lang = self.language;
        let selected = self.selected_home_row();
        let Some(key) = selected.as_ref().and_then(|row| self.key_row(row)) else {
            self.warning(tr!(lang, ClipboardEmpty));
            return;
        };
        let text = public_field(key, fingerprint).to_owned();
        let done = if fingerprint {
            tr!(lang, CopiedFingerprint)
        } else if key.login_type == LOGIN_TYPE_SSH {
            tr!(lang, CopiedPublicKey)
        } else {
            tr!(lang, CopiedUid)
        };
        if text.is_empty() {
            self.warning(tr!(lang, ClipboardEmpty));
            return;
        }
        match crate::clipboard::copy_text(&text) {
            Ok(()) => self.info(done),
            Err(crate::clipboard::ClipboardError::Unsupported) => {
                self.warning(tr!(lang, ClipboardUnsupported));
            }
            Err(crate::clipboard::ClipboardError::Busy) => self.warning(tr!(lang, ClipboardBusy)),
            Err(error) => self.warning(tr!(lang, ClipboardUnavailable, error = error)),
        }
    }
}

/// The only summary field the clipboard may carry: `Y` takes the fingerprint, `y` the
/// public line, which is the SSH public key or the GPG user id. Private material is not
/// part of the summary, so no argument here can reach it.
pub(super) fn public_field(key: &KeyEntrySummary, fingerprint: bool) -> &str {
    if fingerprint {
        &key.fingerprint
    } else if key.login_type == LOGIN_TYPE_SSH {
        &key.public_key
    } else {
        &key.comment
    }
}

/// The one-cell kind a key row shows where a token row shows its provider prefix.
fn key_label(key: &KeyEntrySummary, lang: Language) -> String {
    let fallback = if key.login_type == LOGIN_TYPE_SSH {
        tr!(lang, KeyKindSsh)
    } else {
        tr!(lang, KeyKindGpg)
    };
    if key.algorithm.is_empty() {
        return clean(fallback);
    }
    match key.key_size {
        Some(bits) => format!("{} {bits}", clean(&key.algorithm)),
        None => clean(&key.algorithm),
    }
}

/// Public metadata only. The preview never asks the engine to disclose key text, and the
/// summary it reads was already reduced to these fields when the tree was unlocked.
fn key_fields(lines: &mut Vec<Line<'static>>, key: &KeyEntrySummary, lang: Language) {
    let ssh = key.login_type == LOGIN_TYPE_SSH;
    field(lines, tr!(lang, KeyAlgorithmHeading), key_label(key, lang));
    field(
        lines,
        tr!(lang, KeyFingerprintHeading),
        clean(&key.fingerprint),
    );
    let comment_label = if ssh {
        tr!(lang, KeyCommentHeading)
    } else {
        tr!(lang, KeyUidHeading)
    };
    field(lines, comment_label, clean(&key.comment));
    if ssh {
        field(lines, tr!(lang, KeyPublicHeading), clean(&key.public_key));
    } else {
        field(
            lines,
            tr!(lang, KeyChunksHeading),
            key.chunk_count.to_string(),
        );
    }
    let secret = if key.has_secret {
        tr!(lang, KeyValueYes)
    } else {
        tr!(lang, KeyValueNo)
    };
    field(lines, tr!(lang, KeyPrivateHeading), secret);
    lines.push(Line::default());
    lines.push(Line::raw(tr!(lang, KeyDetailHint)));
    lines.push(Line::raw(tr!(lang, CopyHint)));
}

fn sort_key(row: &HomeRow) -> (String, String) {
    match row {
        HomeRow::Category { path, title, .. } | HomeRow::Entry { path, title, .. } => {
            (path.clone(), title.to_lowercase())
        }
        HomeRow::Action { .. } => (String::new(), String::new()),
    }
}

fn record(app: &App, row: &HomeRow) -> Record {
    let lang = app.language;
    let indent = "  ".repeat(row.depth().min(6));
    match row {
        HomeRow::Category {
            title,
            entries,
            subs,
            id,
            ..
        } => Record {
            name: format!("{indent}{}", clean(title)),
            suffix: if *subs > 0 {
                format!(
                    "{} · {}",
                    counted(lang, *entries, Message::TokenWordOne, Message::TokenWord),
                    counted(lang, *subs, Message::SubWordOne, Message::SubWord)
                )
            } else {
                counted(lang, *entries, Message::TokenWordOne, Message::TokenWord)
            },
            icon: if app.category.as_deref() == Some(id.as_str()) {
                Icon::OpenFolder
            } else {
                Icon::Folder
            },
            color: ACCENT,
        },
        HomeRow::Entry { title, kind, .. } => {
            if let Some(key) = app.key_row(row) {
                return Record {
                    name: format!("{indent}{}", clean(title)),
                    suffix: key_label(key, lang),
                    icon: if key.login_type == LOGIN_TYPE_SSH {
                        Icon::Key
                    } else {
                        Icon::Gpg
                    },
                    color: CYAN,
                };
            }
            let bound = app.home_entry_connection(row);
            Record {
                name: format!("{indent}{}", clean(title)),
                suffix: bound
                    .as_ref()
                    .map_or_else(|| clean(kind), |(_, p)| p.prefix().to_owned()),
                icon: if bound.is_some() {
                    Icon::Grant
                } else {
                    Icon::File
                },
                color: if bound.is_some() { GREEN } else { FG },
            }
        }
        HomeRow::Action { action, title } => Record {
            name: clean(lang.text(*title)),
            suffix: if *action == HomeAction::Authorizations {
                counted(
                    lang,
                    app.grants_in_force(),
                    Message::GrantWordOne,
                    Message::GrantWord,
                )
            } else {
                String::new()
            },
            icon: match action {
                HomeAction::Open => Icon::Unlock,
                HomeAction::OpenLocal => Icon::Database,
                HomeAction::NewDatabase => Icon::File,
                HomeAction::Authorizations => Icon::Grant,
            },
            color: ACCENT,
        },
    }
}

fn database_record(app: &App, index: usize) -> Option<Record> {
    let database = app.databases.get(index)?;
    let unlocked = database.current && app.library.is_some();
    Some(Record {
        name: clean(&database.name),
        suffix: if database.current {
            "●".to_owned()
        } else {
            String::new()
        },
        icon: if !database.current {
            Icon::File
        } else if unlocked {
            Icon::Database
        } else {
            Icon::Unlock
        },
        color: if !database.current {
            DIM
        } else if unlocked {
            GREEN
        } else {
            WARNING
        },
    })
}

fn home_path(app: &App) -> String {
    let lang = app.language;
    let name = app
        .config
        .as_ref()
        .and_then(|config| {
            config
                .database_name
                .as_deref()
                .or_else(|| config.vault.file_stem().and_then(|s| s.to_str()))
        })
        .unwrap_or(lang.text(Message::DefaultDatabase));
    let mut path = format!("monica://{name}");
    if let Some(id) = app.category.as_deref()
        && let Some(library) = &app.library
    {
        path.push_str(" / ");
        path.push_str(&path_of(library, id));
    }
    if app.library.is_some() && !app.home_filter.is_empty() {
        path.push_str(&tr!(
            lang,
            FilterPathSuffix,
            query = app.home_filter.as_str()
        ));
    }
    path
}

pub(super) fn render(frame: &mut Frame<'_>, app: &mut App) {
    let screen = chrome(frame.area());
    let columns = screen.columns;
    path_line(frame, screen.header, &home_path(app));
    databases(frame, app, columns[0]);
    list(
        frame,
        app,
        Rect {
            x: columns[1].x + 1,
            width: columns[1].width.saturating_sub(2),
            ..columns[1]
        },
    );
    detail(frame, app, columns[2]);
    rails(frame, screen.body);
    status(frame, app, screen.footer);
}

fn databases(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let count = app.databases.len();
    let height = area.height as usize;
    let selected = app.home_database_index().unwrap_or(0);
    app.home_rail_offset = if count > height {
        selected.saturating_sub(height - 1)
    } else {
        0
    };
    let offset = app.home_rail_offset;
    for row in 0..height.min(count.saturating_sub(offset)) {
        let index = offset + row;
        let Some(record) = database_record(app, index) else {
            continue;
        };
        draw_record(
            frame,
            Rect {
                y: area.y + row as u16,
                height: 1,
                ..area
            },
            &record,
            app.focus == Focus::Navigation && index == selected,
            app.nerd_font,
        );
    }
}

fn list(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let lang = app.language;
    let rows = app.home_rows();
    app.page_size = area.height.max(1) as usize;
    if rows.is_empty() {
        app.home_offset = 0;
        frame.render_widget(
            Paragraph::new(wrapped_lines(
                vec![
                    heading(tr!(lang, HomeEmptyHeading)),
                    Line::default(),
                    Line::raw(tr!(lang, HomeEmptyAdd)),
                    Line::raw(tr!(lang, HomeEmptySearch)),
                ],
                area.width,
            )),
            area,
        );
        return;
    }
    app.home_selected = app.home_selected.min(rows.len() - 1);
    let height = area.height as usize;
    // Keep the viewport stable until the cursor reaches either scroll margin.
    let margin = 3.min(height / 2);
    if app.home_selected < app.home_offset.saturating_add(margin) {
        app.home_offset = app.home_selected.saturating_sub(margin);
    } else if app.home_selected
        >= app
            .home_offset
            .saturating_add(height)
            .saturating_sub(margin)
    {
        app.home_offset = app
            .home_selected
            .saturating_add(margin + 1)
            .saturating_sub(height);
    }
    app.home_offset = app.home_offset.min(rows.len().saturating_sub(height));
    for (row, index) in (app.home_offset..rows.len()).take(height).enumerate() {
        draw_record(
            frame,
            Rect {
                y: area.y + row as u16,
                height: 1,
                ..area
            },
            &record(app, &rows[index]),
            index == app.home_selected && app.focus != Focus::Navigation,
            app.nerd_font,
        );
    }
}

fn detail(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let inner = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };
    let lines = wrapped_lines(home_detail(app), inner.width);
    app.preview_max_scroll = lines.len().saturating_sub(inner.height as usize);
    app.preview_scroll = app.preview_scroll.min(app.preview_max_scroll);
    frame.render_widget(
        Paragraph::new(lines).scroll((app.preview_scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
    if app.preview_scroll < app.preview_max_scroll {
        frame.render_widget(
            Paragraph::new("↓").style(Style::default().fg(DIM)),
            Rect {
                x: area.right() - 1,
                y: area.bottom() - 1,
                width: 1,
                height: 1,
            },
        );
    }
}

fn home_detail(app: &App) -> Vec<Line<'static>> {
    let lang = app.language;
    let Some(row) = app.selected_home_row() else {
        return vec![
            heading(tr!(lang, HomeEmptyHeading)),
            Line::default(),
            Line::raw(tr!(lang, HomeEmptyAdd)),
        ];
    };
    let mut lines = match &row {
        HomeRow::Category {
            title,
            path,
            entries,
            subs,
            ..
        } => {
            let mut lines = vec![
                heading(title),
                label(if path.is_empty() {
                    tr!(lang, RootCategory).to_owned()
                } else {
                    clean(path)
                }),
            ];
            field(&mut lines, tr!(lang, KindEntry), entries.to_string());
            if *subs > 0 {
                field(&mut lines, tr!(lang, KindSubcategory), subs.to_string());
            }
            lines.push(Line::default());
            lines.push(Line::raw(tr!(lang, DetailOpenCategory)));
            lines
        }
        HomeRow::Entry {
            title,
            kind,
            category,
            path,
            ..
        } => {
            let mut lines = vec![heading(title), label(clean(kind))];
            field(
                &mut lines,
                tr!(lang, KindCategory),
                if path.is_empty() {
                    app.library
                        .as_ref()
                        .and_then(|library| {
                            library
                                .categories
                                .iter()
                                .find(|c| c.id == *category)
                                .map(|c| clean(&c.title))
                        })
                        .unwrap_or_else(|| tr!(lang, RootCategory).to_owned())
                } else {
                    clean(path)
                },
            );
            match app.key_row(&row) {
                Some(entry) => key_fields(&mut lines, entry, lang),
                None => field(&mut lines, tr!(lang, TokenHeading), tr!(lang, TokenMasked)),
            }
            lines
        }
        HomeRow::Action { action, title } => {
            let mut lines = vec![heading(lang.text(*title)), Line::default()];
            if *action == HomeAction::Authorizations {
                lines.push(Line::raw(tr!(lang, GrantsRowDetail)));
                lines.push(Line::default());
            }
            lines.push(Line::raw(tr!(lang, DetailActionHint)));
            lines
        }
    };
    if let Some((name, provider)) = app.home_entry_connection(&row) {
        let Some(connection) = app.config.as_ref().and_then(|c| c.connections.get(&name)) else {
            return lines;
        };
        lines.push(Line::default());
        lines.push(heading(&name));
        lines.push(label(format!(
            "{} · {}",
            provider.prefix(),
            clean(&connection.api_base)
        )));
        lines.push(Line::default());
        lines.push(label(tr!(lang, PurposeHeading)));
        lines.push(Line::raw(if connection.note.is_empty() {
            tr!(lang, NoPurpose).to_owned()
        } else {
            clean(&connection.note)
        }));
        let Some(config) = &app.config else {
            return lines;
        };
        let grants: Vec<_> = config
            .grants
            .iter()
            .filter(|grant| grant.connection == name)
            .collect();
        lines.push(Line::default());
        lines.push(label(tr!(
            lang,
            ProviderGrants,
            provider = provider.prefix(),
            count = grants.len()
        )));
        for grant in grants {
            let state = app.grant_state(grant);
            lines.push(scope_title(grant, &state, lang));
        }
    } else if matches!(row, HomeRow::Entry { .. }) && app.key_row(&row).is_none() {
        lines.push(Line::default());
        lines.push(Line::raw(tr!(lang, NoBindingHint)));
    }
    lines
}

fn context_keys(app: &App) -> &'static str {
    let lang = app.language;
    match app.focus {
        Focus::Navigation => tr!(lang, KeysDatabase),
        Focus::Preview => tr!(lang, KeysPreview),
        Focus::List if app.library.is_none() => tr!(lang, KeysLocked),
        Focus::List => match app.selected_home_row() {
            Some(row @ HomeRow::Entry { .. }) => match app.key_row(&row) {
                Some(key) if key.login_type == LOGIN_TYPE_SSH => tr!(lang, KeysSshEntry),
                Some(_) => tr!(lang, KeysGpgEntry),
                None => tr!(lang, KeysEntry),
            },
            Some(HomeRow::Category { .. }) => tr!(lang, KeysCategory),
            Some(HomeRow::Action { .. }) | None => tr!(lang, KeysAction),
        },
    }
}

/// Drop whole hint groups from the right until the keybar fits, so a narrow
/// terminal never shows half a key binding or a dangling separator.
fn fitted_keys(hints: &str, width: usize) -> String {
    let mut groups: Vec<&str> = hints.split(" · ").collect();
    loop {
        let line = groups.join(" · ");
        if Line::raw(line.clone()).width() <= width {
            return line;
        }
        if groups.len() == 1 {
            return clipped(&line, width, false);
        }
        groups.pop();
    }
}

fn status(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let lang = app.language;
    let (open, close) = if app.nerd_font {
        ("\u{e0b6}", "\u{e0b4}")
    } else {
        (" ", " ")
    };
    let main = Style::default().fg(BG).bg(ACCENT).bold();
    let alternate = Style::default().fg(ACCENT).bg(LIGHT).bold();
    let mode = match &app.mode {
        Mode::Filter(_) => tr!(lang, ModeFilter),
        Mode::Command(_) => tr!(lang, ModeCommand),
        _ => match app.focus {
            Focus::Navigation => tr!(lang, ModeNavigation),
            Focus::List => tr!(lang, ModeNormal),
            Focus::Preview => tr!(lang, ModePreview),
        },
    };
    let kind = match &app.mode {
        Mode::Filter(_) => Message::KindSearch,
        _ => match app.focus {
            Focus::Navigation => Message::KindDatabase,
            _ => app
                .selected_home_row()
                .map_or(Message::KindNone, |row| row.kind()),
        },
    };
    let mut left = vec![
        capsule_edge(open, ACCENT, BG),
        Span::styled(format!(" {mode} "), main),
    ];
    let input = match &app.mode {
        Mode::Command(input) => Some((input, ":")),
        Mode::Filter(filter) => Some((&filter.input, "/")),
        _ => None,
    };
    if let Some((input, prefix)) = input {
        left.push(capsule_edge(close, ACCENT, BG));
        let width = Line::from(left.clone()).width() as u16;
        frame.render_widget(Paragraph::new(Line::from(left)), area);
        render_input(
            frame,
            input,
            prefix,
            Rect {
                x: area.x + width + 1,
                width: area.width.saturating_sub(width + 1),
                ..area
            },
        );
        return;
    }
    left.extend([
        capsule_edge(close, ACCENT, LIGHT),
        Span::styled(format!(" {}", lang.text(kind)), alternate),
        capsule_edge(close, LIGHT, BG),
    ]);
    let rows = app.home_rows().len();
    let index = if rows == 0 {
        0
    } else {
        app.home_selected.min(rows - 1) + 1
    };
    let (state, color) = broker_status(app);
    let right = Line::from(vec![
        Span::styled(
            format!(
                " {} ",
                if rows == 0 {
                    "0".to_owned()
                } else {
                    format!("{index}/{rows}")
                }
            ),
            main,
        ),
        capsule_edge(open, ACCENT, LIGHT),
        Span::styled(
            format!(
                " {} ",
                position(index.saturating_sub(1), rows.saturating_sub(1), lang)
            ),
            alternate,
        ),
        capsule_edge(open, LIGHT, BG),
        Span::styled(format!(" {state} "), Style::default().fg(color)),
    ]);
    let right_width = right.width() as u16;
    let left = Line::from(left);
    let left_width = left.width() as u16;
    frame.render_widget(
        Paragraph::new(left),
        Rect {
            width: left_width,
            ..area
        },
    );
    frame.render_widget(
        Paragraph::new(right),
        Rect {
            x: area.right() - right_width,
            width: right_width,
            ..area
        },
    );
    let middle = Rect {
        x: area.x + left_width,
        width: area.width.saturating_sub(left_width + right_width),
        ..area
    };
    let (text, color) = if app.pending.is_some() {
        (lang.text(app.pending_label).to_owned(), WARNING)
    } else if app
        .message_at
        .is_some_and(|time| app.failed || time.elapsed().as_secs() < 6)
    {
        (
            format!("! {}", app.message),
            if app.failed { ERROR } else { FG },
        )
    } else {
        (
            fitted_keys(
                &format!("{} · {}", context_keys(app), tr!(lang, KeysTail)),
                middle.width.saturating_sub(1) as usize,
            ),
            DIM,
        )
    };
    frame.render_widget(
        Paragraph::new(format!(
            " {}",
            clipped(&text, middle.width.saturating_sub(1) as usize, false)
        ))
        .style(Style::default().fg(color)),
        middle,
    );
}
