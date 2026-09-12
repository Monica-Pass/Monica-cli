//! Pure browsing state. A filtered row is resolved to its source record before
//! any management action; previews and filters never open the vault.
use zeroize::Zeroizing;

use super::form::Input;
use super::{App, COMMANDS, CommandHelp, Mode, Page};
use crate::config::Grant;
use crate::i18n::Message;
use crate::model::Operation;
use crate::tr;

pub(super) const QUICK_COMMANDS: [&str; 5] = ["add", "open", "login", "grant", "unlock"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Navigation,
    List,
    Preview,
}

pub(super) struct Filter {
    pub input: Input,
    pub previous: Zeroizing<String>,
    pub selection: Option<String>,
}

pub(super) fn quick_command(index: usize) -> Option<&'static CommandHelp> {
    let name = QUICK_COMMANDS.get(index)?;
    COMMANDS.iter().find(|command| command.command == *name)
}

pub(super) fn grant_status(grant: &Grant) -> Message {
    let now = chrono::Utc::now().timestamp();
    if now < grant.issued_at {
        Message::GrantPending
    } else if grant.expires_at != 0 && now >= grant.expires_at {
        Message::GrantExpired
    } else {
        Message::GrantActive
    }
}

pub(super) fn grant_access(grant: &Grant) -> Message {
    if grant
        .operations
        .iter()
        .any(|operation| operation.is_write())
    {
        if grant.operations.contains(&Operation::ListIssues)
            || grant.operations.contains(&Operation::GetIssue)
            || grant.operations.contains(&Operation::ApiRead)
        {
            Message::AccessReadWrite
        } else {
            Message::AccessWriteOnly
        }
    } else {
        Message::AccessReadOnly
    }
}

impl App {
    pub(super) fn total_rows(&self, page: Page) -> usize {
        match page {
            Page::Dashboard => QUICK_COMMANDS.len(),
            Page::Connections => self
                .config
                .as_ref()
                .map_or(0, |config| config.connections.len()),
            Page::Grants => self.config.as_ref().map_or(0, |config| config.grants.len()),
            Page::WebDav => self.entries.len(),
            Page::Help => COMMANDS.len(),
        }
    }

    pub(super) fn visible_rows(&self, page: Page) -> Vec<usize> {
        let lang = self.language;
        let query = self.filters[page.index()].to_lowercase();
        if query.trim().is_empty() {
            return (0..self.total_rows(page)).collect();
        }
        // Explicit public fields only. Do not serialize Connection/Grant: their
        // internal IDs and capability hashes are not browsing/search content.
        (0..self.total_rows(page))
            .filter(|index| {
                let text = match page {
                    Page::Dashboard | Page::Help => {
                        let command = if page == Page::Dashboard {
                            quick_command(*index)
                        } else {
                            COMMANDS.get(*index)
                        };
                        command.map_or_else(String::new, |command| {
                            format!(
                                "{} {} {} {}",
                                command.command,
                                command.key,
                                lang.text(command.description),
                                lang.text(command.cli)
                            )
                        })
                    }
                    Page::Connections => self
                        .config
                        .as_ref()
                        .and_then(|config| config.connections.iter().nth(*index))
                        .map_or_else(String::new, |(name, connection)| {
                            format!(
                                "{name} {} {} {}",
                                connection.provider.prefix(),
                                connection.note,
                                connection.api_base
                            )
                        }),
                    Page::Grants => self
                        .config
                        .as_ref()
                        .and_then(|config| config.grants.get(*index))
                        .map_or_else(String::new, |grant| {
                            format!(
                                "{} {} {} {} {} {}",
                                grant.name,
                                grant.connection,
                                grant
                                    .repositories
                                    .iter()
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(" "),
                                grant
                                    .operations
                                    .iter()
                                    .map(|operation| operation.name().replace('_', "-"))
                                    .collect::<Vec<_>>()
                                    .join(" "),
                                lang.text(grant_status(grant)),
                                lang.text(grant_access(grant))
                            )
                        }),
                    Page::WebDav => self.entries.get(*index).map_or_else(String::new, |entry| {
                        format!(
                            "{} {}",
                            entry.path,
                            if entry.is_directory {
                                tr!(lang, SearchFolder)
                            } else {
                                tr!(lang, SearchFile)
                            }
                        )
                    }),
                }
                .to_lowercase();
                query.split_whitespace().all(|part| text.contains(part))
            })
            .collect()
    }

    pub(super) fn selected_index(&self, page: Page) -> Option<usize> {
        self.visible_rows(page)
            .get(self.selected[page.index()])
            .copied()
    }

    fn row_key(&self, page: Page, index: usize) -> Option<String> {
        match page {
            Page::Dashboard => quick_command(index).map(|command| command.command.to_owned()),
            Page::Connections => self.config.as_ref()?.connections.keys().nth(index).cloned(),
            Page::Grants => self
                .config
                .as_ref()?
                .grants
                .get(index)
                .map(|grant| grant.name.clone()),
            Page::WebDav => self.entries.get(index).map(|entry| entry.path.clone()),
            Page::Help => COMMANDS
                .get(index)
                .map(|command| command.command.to_owned()),
        }
    }

    pub(super) fn selection_key(&self, page: Page) -> Option<String> {
        self.row_key(page, self.selected_index(page)?)
    }

    pub(super) fn restore_selection(&mut self, page: Page, key: Option<&str>) {
        self.selected[page.index()] = key
            .and_then(|key| {
                self.visible_rows(page)
                    .iter()
                    .position(|index| self.row_key(page, *index).as_deref() == Some(key))
            })
            .unwrap_or(0);
    }

    pub(super) fn rows(&self) -> usize {
        self.visible_rows(self.page).len()
    }

    pub(super) fn clamp_selection(&mut self) {
        for page in Page::ALL {
            self.selected[page.index()] =
                self.selected[page.index()].min(self.visible_rows(page).len().saturating_sub(1));
        }
    }

    pub(super) fn set_page(&mut self, page: Page) {
        self.home = false;
        if matches!(self.mode, Mode::Filter(_)) {
            self.mode = Mode::Normal;
        }
        self.page = page;
        self.focus = Focus::List;
        self.preview_scroll = 0;
        self.clamp_selection();
    }

    pub(super) fn shift_focus(&mut self, forward: bool) {
        self.focus = match (self.focus, forward) {
            (Focus::Navigation, true) | (Focus::Preview, false) => Focus::List,
            (Focus::List, true) | (Focus::Navigation, false) => Focus::Preview,
            (Focus::List, false) | (Focus::Preview, true) => Focus::Navigation,
        };
    }

    pub(super) fn move_selection(&mut self, down: bool, step: usize) {
        match self.focus {
            Focus::Navigation => {
                let index = if down {
                    self.page.index().saturating_add(step).min(4)
                } else {
                    self.page.index().saturating_sub(step)
                };
                self.set_page(Page::ALL[index]);
                self.focus = Focus::Navigation;
            }
            Focus::List => {
                let max = self.rows().saturating_sub(1);
                let selection = &mut self.selected[self.page.index()];
                *selection = if down {
                    selection.saturating_add(step).min(max)
                } else {
                    selection.saturating_sub(step)
                };
                self.preview_scroll = 0;
            }
            Focus::Preview => {
                self.preview_scroll = if down {
                    self.preview_scroll
                        .saturating_add(step)
                        .min(self.preview_max_scroll)
                } else {
                    self.preview_scroll.saturating_sub(step)
                };
            }
        }
    }

    pub(super) fn begin_filter(&mut self) {
        self.focus = Focus::List;
        self.mode = Mode::Filter(Filter {
            input: Input::new(&self.filters[self.page.index()], 256),
            previous: self.filters[self.page.index()].clone(),
            selection: self.selection_key(self.page),
        });
    }

    pub(super) fn update_filter(&mut self, text: &str) {
        let selected = self.selection_key(self.page);
        self.filters[self.page.index()] = Zeroizing::new(text.to_owned());
        self.restore_selection(self.page, selected.as_deref());
        self.preview_scroll = 0;
    }

    pub(super) fn paste(&mut self, text: &str) {
        let mode = std::mem::replace(&mut self.mode, Mode::Normal);
        match mode {
            Mode::Form(mut form) => {
                form.paste(text);
                self.mode = Mode::Form(form);
            }
            Mode::Filter(mut filter) => {
                filter.input.insert(text);
                self.update_filter(&filter.input.value);
                self.mode = Mode::Filter(filter);
            }
            mode => self.mode = mode,
        }
    }
}
