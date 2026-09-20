use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use zeroize::Zeroizing;

use super::App;
use super::actions::Action;
use crate::admin::{AddOptions, GrantOptions, validate_new_password};
use crate::config::DEFAULT_PORT;
use crate::error::{GatewayError, Result};
use crate::model::{
    Operation, Provider, validate_api_base, validate_name, validate_note, validate_repository,
    validate_title,
};
use crate::tr;
use crate::webdav::{WebDavProfile, normalize_path};

pub(super) struct Input {
    pub value: Zeroizing<String>,
    pub cursor: usize,
    pub limit: usize,
    /// Key material is pasted as armor or PEM, where the line breaks belong to the value.
    pub multiline: bool,
}

impl Input {
    pub fn new(value: &str, limit: usize) -> Self {
        let mut text = String::with_capacity(limit);
        text.extend(value.chars().filter(|ch| *ch != '\n' && !ch.is_control()));
        while text.len() > limit {
            text.pop();
        }
        Self {
            cursor: text.len(),
            value: Zeroizing::new(text),
            limit,
            multiline: false,
        }
    }

    pub fn insert(&mut self, text: &str) {
        let multiline = self.multiline;
        for ch in text
            .chars()
            .filter(|ch| (multiline && *ch == '\n') || !ch.is_control())
        {
            if self.value.len() + ch.len_utf8() > self.limit {
                break;
            }
            self.value.insert(self.cursor, ch);
            self.cursor += ch.len_utf8();
        }
    }

    /// How many lines landed, so a masked field can still prove the paste was not truncated.
    pub fn lines(&self) -> usize {
        if self.multiline {
            self.value.lines().count()
        } else {
            0
        }
    }

    pub fn key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                use zeroize::Zeroize;
                self.value.zeroize();
                self.cursor = 0;
            }
            KeyCode::Enter if self.multiline => self.insert("\n"),
            KeyCode::Char(ch)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert(&ch.to_string())
            }
            KeyCode::Left => {
                self.cursor = self.value[..self.cursor]
                    .char_indices()
                    .last()
                    .map_or(0, |(position, _)| position)
            }
            KeyCode::Right => {
                if let Some(ch) = self.value[self.cursor..].chars().next() {
                    self.cursor += ch.len_utf8();
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.len(),
            KeyCode::Backspace if self.cursor > 0 => {
                let previous = self.value[..self.cursor]
                    .char_indices()
                    .last()
                    .map_or(0, |(position, _)| position);
                self.value.remove(previous);
                self.cursor = previous;
            }
            KeyCode::Delete if self.cursor < self.value.len() => {
                self.value.remove(self.cursor);
            }
            _ => {}
        }
    }
}

pub(super) struct Field {
    pub label: &'static str,
    pub hint: &'static str,
    pub secret: bool,
    pub input: Input,
}

impl Field {
    fn text(label: &'static str, value: &str, hint: &'static str) -> Self {
        Self {
            label,
            hint,
            secret: false,
            input: Input::new(value, 4096),
        }
    }
    fn secret(label: &'static str) -> Self {
        Self {
            label,
            hint: "",
            secret: true,
            input: Input::new("", 4096),
        }
    }
    /// Pasted armor or PEM: masked like a password, but it keeps the line breaks it arrives with.
    fn key_text(label: &'static str, hint: &'static str) -> Self {
        let mut input = Input::new("", crate::keys::limits::MAX_KEY_INPUT_BYTES);
        input.multiline = true;
        Self {
            label,
            hint,
            secret: true,
            input,
        }
    }
    pub fn display(&self) -> String {
        if self.secret {
            "*".repeat(self.input.value.chars().count())
        } else {
            self.input.value.to_string()
        }
    }
}

#[derive(Clone)]
pub(super) enum Kind {
    SwitchDatabase {
        id: String,
        name: String,
    },
    Token(String),
    RenameCategory {
        id: String,
        title: String,
    },
    RenameEntry {
        name: String,
        title: String,
    },
    Category(Option<String>),
    Move(String),
    Library,
    Add {
        new_vault: bool,
    },
    Note,
    Init,
    OpenLocal,
    Connect,
    Grant,
    Login,
    OpenRemote(String),
    Publish,
    Unlock,
    Sync,
    Revoke,
    AddSsh {
        generate: bool,
    },
    AddGpg,
    EditKey {
        entry_id: String,
        login_type: String,
        title: String,
        comment: String,
        notes: String,
    },
}

pub(super) enum FormEvent {
    None,
    Cancel,
    Submit,
}

pub(super) struct Form {
    pub kind: Kind,
    pub title: &'static str,
    pub notice: String,
    pub fields: Vec<Field>,
    pub selected: usize,
    pub insert: bool,
    pub auth_field: Option<usize>,
    pub authenticating: bool,
}

impl Form {
    pub fn new(kind: Kind, app: &App) -> Self {
        let lang = app.language;
        let vault_path = app
            .store
            .path
            .with_file_name(if app.config.is_some() {
                "new-database.mdbx"
            } else {
                "gateway.mdbx"
            })
            .display()
            .to_string();
        let name = app.selected_connection().unwrap_or_default();
        let profile = app
            .config
            .as_ref()
            .and_then(|config| config.webdav.as_ref())
            .map(|binding| binding.profile.clone())
            .or_else(|| app.profile.clone());
        let (title, notice, mut fields) = match &kind {
            Kind::SwitchDatabase { name, .. } => (
                if lang == crate::i18n::Language::En {
                    "Open database"
                } else {
                    "打开数据库"
                },
                if lang == crate::i18n::Language::En {
                    format!("Unlock {name}. AI grants must be created again after switching.")
                } else {
                    format!("解锁 {name}。切换后需重新创建 AI 授权。")
                },
                vec![Field::secret(tr!(lang, MasterPasswordLabel))],
            ),
            Kind::Token(name) => (
                if lang == crate::i18n::Language::En {
                    "Update Token"
                } else {
                    "更新 Token"
                },
                if lang == crate::i18n::Language::En {
                    format!(
                        "{name}: replace the encrypted Token. Previous AI grants will be revoked."
                    )
                } else {
                    format!("更新 {name} 的加密 Token，原有 AI 授权将撤销。")
                },
                vec![
                    Field::secret(tr!(lang, ServiceTokenLabel)),
                    Field::secret(tr!(lang, MasterPasswordLabel)),
                ],
            ),
            Kind::RenameCategory { title, .. } => (
                if lang == crate::i18n::Language::En {
                    "Rename category"
                } else {
                    "重命名分类"
                },
                String::new(),
                vec![
                    Field::text(
                        if lang == crate::i18n::Language::En {
                            "Name"
                        } else {
                            "分类名称"
                        },
                        title,
                        "",
                    ),
                    Field::secret(tr!(lang, MasterPasswordLabel)),
                ],
            ),
            Kind::RenameEntry { title, .. } => (
                if lang == crate::i18n::Language::En {
                    "Rename entry"
                } else {
                    "重命名条目"
                },
                if lang == crate::i18n::Language::En {
                    "Set a display title (Chinese allowed); the AI handle is unchanged.".to_owned()
                } else {
                    "设置显示名称（支持中文）；AI 句柄保持不变。".to_owned()
                },
                vec![
                    Field::text(
                        tr!(lang, DisplayTitleLabel),
                        title,
                        tr!(lang, DisplayTitleHint),
                    ),
                    Field::secret(tr!(lang, MasterPasswordLabel)),
                ],
            ),
            Kind::Category(_) => (
                if lang == crate::i18n::Language::En {
                    "New category"
                } else {
                    "新建分类"
                },
                if lang == crate::i18n::Language::En {
                    "Create inside the current category."
                } else {
                    "在当前分类下创建子分类。"
                }
                .to_owned(),
                vec![
                    Field::text(
                        if lang == crate::i18n::Language::En {
                            "Name"
                        } else {
                            "分类名称"
                        },
                        "",
                        "",
                    ),
                    Field::secret(tr!(lang, MasterPasswordLabel)),
                ],
            ),
            Kind::Move(_) => (
                if lang == crate::i18n::Language::En {
                    "Move to category"
                } else {
                    "移动到分类"
                },
                if lang == crate::i18n::Language::En {
                    "Enter an exact category name or ID."
                } else {
                    "输入目标分类名称；重名时使用分类 ID。"
                }
                .to_owned(),
                vec![
                    Field::text(
                        if lang == crate::i18n::Language::En {
                            "Destination"
                        } else {
                            "目标分类"
                        },
                        "",
                        "",
                    ),
                    Field::secret(tr!(lang, MasterPasswordLabel)),
                ],
            ),
            Kind::Library => (
                if lang == crate::i18n::Language::En {
                    "Open database"
                } else {
                    "打开数据库"
                },
                if lang == crate::i18n::Language::En {
                    "Unlock to browse your categories and entries."
                } else {
                    "输入当前数据库的密码，查看分类和条目。"
                }
                .to_owned(),
                vec![Field::secret(tr!(lang, MasterPasswordLabel))],
            ),
            Kind::Add { new_vault } => {
                let mut fields = vec![
                    Field::text(
                        tr!(lang, ConnectionNameForAi),
                        "",
                        tr!(lang, ConnectionNameHint),
                    ),
                    Field::text(
                        tr!(lang, DisplayTitleLabel),
                        "",
                        tr!(lang, DisplayTitleHint),
                    ),
                    Field::text(tr!(lang, ProviderLabel), "github", tr!(lang, ProviderHint)),
                    Field::text(
                        tr!(lang, RepositoriesLabel),
                        "",
                        tr!(lang, RepositoriesHint),
                    ),
                    Field::text(tr!(lang, OptionalNoteLabel), "", tr!(lang, PurposeNoteHint)),
                    Field::text(tr!(lang, OptionalApiBaseLabel), "", tr!(lang, ApiBaseHint)),
                    Field::secret(tr!(lang, ServiceTokenLabel)),
                    Field::secret(if *new_vault {
                        tr!(lang, NewPasswordLabel)
                    } else {
                        tr!(lang, MasterPasswordLabel)
                    }),
                ];
                if *new_vault {
                    fields.push(Field::secret(tr!(lang, ConfirmPasswordLabel)));
                }
                (
                    tr!(lang, QuickAddTitle),
                    if *new_vault {
                        tr!(lang, QuickAddFirstNotice).to_owned()
                    } else {
                        tr!(lang, QuickAddNotice).to_owned()
                    },
                    fields,
                )
            }
            Kind::Note => {
                let note = app
                    .config
                    .as_ref()
                    .and_then(|config| config.connections.get(&name))
                    .map_or("", |binding| binding.note.as_str());
                (
                    tr!(lang, EditNoteTitle),
                    tr!(lang, EditNoteNotice).to_owned(),
                    vec![
                        Field::text(
                            tr!(lang, ConnectionNameLabel),
                            &name,
                            tr!(lang, ExistingConnectionHint),
                        ),
                        Field::text(tr!(lang, NoteLabel), note, tr!(lang, NoteLimitHint)),
                        Field::secret(tr!(lang, MasterPasswordLabel)),
                    ],
                )
            }
            Kind::Init => (
                tr!(lang, CreateVaultTitle),
                tr!(lang, NewPasswordNotice).to_owned(),
                vec![
                    Field::text(
                        tr!(lang, NewLocalFileLabel),
                        &vault_path,
                        tr!(lang, NewLocalFileHint),
                    ),
                    Field::text(
                        tr!(lang, BrokerPortLabel),
                        &DEFAULT_PORT.to_string(),
                        tr!(lang, BrokerPortHint),
                    ),
                    Field::secret(tr!(lang, NewPasswordLabel)),
                    Field::secret(tr!(lang, ConfirmPasswordLabel)),
                ],
            ),
            Kind::OpenLocal => (
                tr!(lang, OpenLocalTitle),
                tr!(lang, OpenLocalNotice).to_owned(),
                vec![
                    Field::text(tr!(lang, MdbxFileLabel), "", tr!(lang, LocalPathHint)),
                    Field::secret(tr!(lang, MasterPasswordLabel)),
                ],
            ),
            Kind::Connect => (
                tr!(lang, ConnectTitle),
                tr!(lang, ConnectNotice).to_owned(),
                vec![
                    Field::text(tr!(lang, ConnectionNameLabel), "", tr!(lang, ShortNameHint)),
                    Field::text(
                        tr!(lang, DisplayTitleLabel),
                        "",
                        tr!(lang, DisplayTitleHint),
                    ),
                    Field::text(tr!(lang, ProviderLabel), "github", tr!(lang, ProviderHint)),
                    Field::text(tr!(lang, ApiBaseLabel), "", tr!(lang, BlankApiBaseHint)),
                    Field::text(tr!(lang, OptionalNoteLabel), "", tr!(lang, NoSecretsHint)),
                    Field::secret(tr!(lang, ServiceTokenLabel)),
                    Field::secret(tr!(lang, MasterPasswordLabel)),
                ],
            ),
            Kind::Grant => (
                tr!(lang, CreateGrantTitle),
                tr!(lang, GrantNotice).to_owned(),
                vec![
                    Field::text(tr!(lang, GrantNameLabel), "", tr!(lang, GrantNameHint)),
                    Field::text(
                        tr!(lang, PageConnections),
                        &name,
                        tr!(lang, GrantConnectionHint),
                    ),
                    Field::text(
                        tr!(lang, RepositoriesLabel),
                        "",
                        tr!(lang, ExactRepositoriesHint),
                    ),
                    Field::text(
                        tr!(lang, OperationsLabel),
                        "list-issues,get-issue",
                        "list-issues,get-issue,create-issue / api-read,api-write",
                    ),
                    Field::text(
                        tr!(lang, TtlLabel),
                        "",
                        if lang == crate::i18n::Language::En {
                            "Blank = 240 minutes; 1–1440 allowed"
                        } else {
                            "留空即 240 分钟；可填 1–1440"
                        },
                    ),
                    Field::text(tr!(lang, RateLimitLabel), "60", "1–600"),
                    Field::secret(tr!(lang, MasterPasswordLabel)),
                ],
            ),
            Kind::Login => (
                tr!(lang, WebDavLoginTitle),
                tr!(lang, WebDavLoginNotice).to_owned(),
                vec![
                    Field::text(
                        tr!(lang, WebDavUrlLabel),
                        profile.as_ref().map_or("", |profile| &profile.base_url),
                        tr!(lang, WebDavUrlHint),
                    ),
                    Field::text(
                        tr!(lang, UsernameLabel),
                        profile.as_ref().map_or("", |profile| &profile.username),
                        tr!(lang, UsernameHint),
                    ),
                    Field::secret(tr!(lang, WebDavPasswordLabel)),
                ],
            ),
            Kind::OpenRemote(path) => (
                tr!(lang, OpenRemoteTitle),
                tr!(lang, OpenRemoteNotice, path = path),
                vec![Field::secret(tr!(lang, RemotePasswordLabel))],
            ),
            Kind::Publish => {
                let path = if app.folder.is_empty() {
                    "gateway.mdbx".to_owned()
                } else {
                    format!("{}/gateway.mdbx", app.folder)
                };
                (
                    tr!(lang, PublishTitle),
                    tr!(lang, PublishNotice).to_owned(),
                    vec![
                        Field::text(
                            tr!(lang, NewRemoteFileLabel),
                            &path,
                            tr!(lang, RemotePathHint),
                        ),
                        Field::secret(tr!(lang, MasterPasswordLabel)),
                    ],
                )
            }
            Kind::Unlock => (
                tr!(lang, UnlockTitle),
                tr!(lang, UnlockNotice).to_owned(),
                vec![Field::secret(tr!(lang, MasterPasswordLabel))],
            ),
            Kind::Sync => (
                tr!(lang, SyncTitle),
                tr!(lang, SyncNotice).to_owned(),
                vec![Field::secret(tr!(lang, MasterPasswordLabel))],
            ),
            Kind::Revoke => (
                tr!(lang, RevokeTitle),
                tr!(lang, RevokeNotice).to_owned(),
                vec![Field::text(
                    tr!(lang, GrantNameLabel),
                    &app.selected_grant()
                        .map_or_else(String::new, |grant| grant.name.clone()),
                    tr!(lang, RevokeHint),
                )],
            ),
            Kind::AddSsh { generate } => {
                let mut fields = vec![Field::text(
                    tr!(lang, KeyNameLabel),
                    "",
                    tr!(lang, KeyNameHint),
                )];
                let (notice, title) = if *generate {
                    fields.extend([
                        Field::text(
                            tr!(lang, KeyAlgorithmLabel),
                            "ed25519",
                            tr!(lang, KeyAlgorithmHint),
                        ),
                        Field::text(tr!(lang, KeyCommentLabel), "", tr!(lang, KeyCommentHint)),
                    ]);
                    (
                        tr!(lang, KeyGenerateNotice).to_owned(),
                        tr!(lang, AddSshTitle),
                    )
                } else {
                    fields.push(Field::key_text(
                        tr!(lang, KeyPrivateLabel),
                        tr!(lang, KeyPrivateHint),
                    ));
                    (
                        tr!(lang, KeyImportNotice).to_owned(),
                        tr!(lang, ImportSshTitle),
                    )
                };
                fields.extend([
                    Field::text(tr!(lang, KeyNoteLabel), "", tr!(lang, KeyNoteHint)),
                    Field::secret(tr!(lang, MasterPasswordLabel)),
                ]);
                (title, notice, fields)
            }
            Kind::AddGpg => (
                tr!(lang, AddGpgTitle),
                tr!(lang, KeyGpgNotice).to_owned(),
                vec![
                    Field::text(tr!(lang, KeyNameLabel), "", tr!(lang, KeyNameHint)),
                    Field::key_text(tr!(lang, KeyArmorLabel), tr!(lang, KeyArmorHint)),
                    Field::text(tr!(lang, KeyNoteLabel), "", tr!(lang, KeyNoteHint)),
                    Field::secret(tr!(lang, MasterPasswordLabel)),
                ],
            ),
            Kind::EditKey {
                login_type,
                title,
                comment,
                notes,
                ..
            } => {
                let mut fields = vec![Field::text(
                    tr!(lang, KeyNameLabel),
                    title,
                    tr!(lang, KeyNameHint),
                )];
                if login_type == crate::keys::payload::LOGIN_TYPE_SSH {
                    fields.push(Field::text(
                        tr!(lang, KeyCommentLabel),
                        comment,
                        tr!(lang, KeyCommentHint),
                    ));
                }
                fields.extend([
                    Field::text(tr!(lang, KeyNoteLabel), notes, tr!(lang, KeyNoteHint)),
                    Field::secret(tr!(lang, MasterPasswordLabel)),
                ]);
                (
                    tr!(lang, EditKeyTitle),
                    tr!(lang, KeyEditNotice).to_owned(),
                    fields,
                )
            }
        };
        for field in &mut fields {
            // A pasted armor keeps its own instructions; the hidden-input note is for passwords.
            if field.secret && !field.input.multiline {
                field.hint = tr!(lang, HiddenInputHint);
            }
        }
        let auth_field = match kind {
            Kind::Token(_)
            | Kind::RenameCategory { .. }
            | Kind::RenameEntry { .. }
            | Kind::Category(_)
            | Kind::Move(_)
            | Kind::Add { new_vault: false }
            | Kind::Note
            | Kind::Connect
            | Kind::Grant
            | Kind::Publish
            | Kind::AddSsh { .. }
            | Kind::AddGpg
            | Kind::EditKey { .. }
            | Kind::OpenLocal => Some(fields.len() - 1),
            _ => None,
        };
        Self {
            kind,
            title,
            notice,
            fields,
            selected: 0,
            insert: true,
            auth_field,
            authenticating: false,
        }
    }

    pub fn visible_fields(&self) -> Vec<usize> {
        if self.authenticating {
            return self.auth_field.into_iter().collect();
        }
        (0..self.fields.len())
            .filter(|i| Some(*i) != self.auth_field)
            .collect()
    }

    fn submit(&mut self) -> FormEvent {
        if let Some(index) = self.auth_field
            && !self.authenticating
        {
            self.authenticating = true;
            self.selected = index;
            self.insert = true;
            return FormEvent::None;
        }
        FormEvent::Submit
    }

    pub fn key(&mut self, key: KeyEvent) -> FormEvent {
        if key.code == KeyCode::Esc && self.authenticating {
            if let Some(index) = self.auth_field {
                self.fields[index].input = Input::new("", 4096);
            }
            self.authenticating = false;
            self.selected = 0;
            self.insert = true;
            return FormEvent::None;
        }
        let indices = self.visible_fields();
        let position = indices
            .iter()
            .position(|i| *i == self.selected)
            .unwrap_or(0);
        match key.code {
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                return self.submit();
            }
            KeyCode::Esc if self.insert => self.insert = false,
            KeyCode::Esc => return FormEvent::Cancel,
            KeyCode::Enter if self.insert && self.fields[self.selected].input.multiline => {
                self.fields[self.selected].input.key(key);
            }
            KeyCode::Tab | KeyCode::Down => self.selected = indices[(position + 1) % indices.len()],
            KeyCode::BackTab | KeyCode::Up => {
                self.selected = indices[position.checked_sub(1).unwrap_or(indices.len() - 1)]
            }
            KeyCode::Enter if position + 1 == indices.len() => return self.submit(),
            KeyCode::Enter => {
                self.selected = indices[position + 1];
                self.insert = true;
            }
            KeyCode::Char('j') if !self.insert => {
                self.selected = indices[(position + 1) % indices.len()]
            }
            KeyCode::Char('k') if !self.insert => {
                self.selected = indices[position.checked_sub(1).unwrap_or(indices.len() - 1)]
            }
            KeyCode::Char('i') if !self.insert => self.insert = true,
            KeyCode::Char('q') if !self.insert => return FormEvent::Cancel,
            _ if self.insert => self.fields[self.selected].input.key(key),
            _ => {}
        }
        FormEvent::None
    }

    pub fn paste(&mut self, value: &str) {
        if self.insert {
            self.fields[self.selected].input.insert(value);
        }
    }

    fn text(&self, index: usize) -> &str {
        self.fields[index].input.value.trim()
    }
    fn secret(&mut self, index: usize) -> Zeroizing<String> {
        std::mem::replace(
            &mut self.fields[index].input.value,
            Zeroizing::new(String::new()),
        )
    }

    pub fn action(&mut self, app: &App) -> Result<Action> {
        Ok(match &self.kind {
            Kind::SwitchDatabase { id, .. } => Action::SwitchDatabase {
                id: id.clone(),
                password: self.secret(0),
            },
            Kind::Token(name) => Action::Token {
                name: name.clone(),
                token: self.secret(0),
                password: self.secret(1),
            },
            Kind::RenameCategory { id, .. } => Action::RenameCategory {
                id: id.clone(),
                title: self.text(0).to_owned(),
                password: self.secret(1),
            },
            Kind::RenameEntry { name, .. } => Action::RenameEntry {
                name: name.clone(),
                title: self.text(0).to_owned(),
                password: self.secret(1),
            },
            Kind::Category(parent) => Action::Category {
                title: self.text(0).to_owned(),
                parent: parent.clone(),
                password: self.secret(1),
            },
            Kind::Move(id) => {
                let library = app.library.as_ref().ok_or(GatewayError::UnlockRequired)?;
                let matches: Vec<_> = library
                    .categories
                    .iter()
                    .filter(|c| c.id == self.text(0) || c.title == self.text(0))
                    .collect();
                if matches.len() != 1 {
                    return Err(GatewayError::InvalidRequest);
                }
                Action::Move {
                    id: id.clone(),
                    target: matches[0].id.clone(),
                    password: self.secret(1),
                }
            }
            Kind::Library => Action::Library(self.secret(0)),
            Kind::Add { new_vault } => {
                let creating = *new_vault;
                let provider = match self.text(2) {
                    "github" => Provider::Github,
                    "gitlab" => Provider::Gitlab,
                    _ => return Err(GatewayError::InvalidRequest),
                };
                let options = AddOptions {
                    name: self.text(0).to_owned(),
                    title: self.text(1).to_owned(),
                    provider,
                    repositories: self
                        .text(3)
                        .split(',')
                        .map(str::trim)
                        .map(str::to_owned)
                        .collect(),
                    note: self.text(4).to_owned(),
                    api_base: (!self.text(5).is_empty()).then(|| self.text(5).to_owned()),
                    allow_write: false,
                    ttl_minutes: 0,
                };
                options.validate()?;
                if creating {
                    validate_new_password(
                        &self.fields[7].input.value,
                        &self.fields[8].input.value,
                    )?;
                }
                Action::Add {
                    options,
                    token: self.secret(6),
                    password: self.secret(7),
                    confirmation: if creating { Some(self.secret(8)) } else { None },
                }
            }
            Kind::Note => {
                validate_name(self.text(0))?;
                validate_note(self.text(1))?;
                Action::Note {
                    name: self.text(0).to_owned(),
                    note: self.text(1).to_owned(),
                    password: self.secret(2),
                }
            }
            Kind::Init => {
                let path = PathBuf::from(self.text(0));
                let port = self
                    .text(1)
                    .parse::<u16>()
                    .ok()
                    .filter(|port| *port >= 1024)
                    .ok_or(GatewayError::InvalidConfig)?;
                validate_new_password(&self.fields[2].input.value, &self.fields[3].input.value)?;
                Action::Init {
                    path,
                    port,
                    password: self.secret(2),
                    confirmation: self.secret(3),
                }
            }
            Kind::OpenLocal => Action::OpenLocal {
                path: PathBuf::from(self.text(0)),
                password: self.secret(1),
            },
            Kind::Connect => {
                validate_name(self.text(0))?;
                let provider = match self.text(2) {
                    "github" => Provider::Github,
                    "gitlab" => Provider::Gitlab,
                    _ => return Err(GatewayError::InvalidRequest),
                };
                let base = validate_api_base(
                    if self.text(3).is_empty() {
                        provider.default_api_base()
                    } else {
                        self.text(3)
                    },
                    provider,
                )?
                .to_string();
                Action::Connect {
                    category: if app.home { app.category.clone() } else { None },
                    name: self.text(0).to_owned(),
                    title: self.text(1).to_owned(),
                    provider,
                    base,
                    note: self.text(4).to_owned(),
                    token: self.secret(5),
                    password: self.secret(6),
                }
            }
            Kind::Grant => {
                validate_name(self.text(0))?;
                let config = app.config.as_ref().ok_or(GatewayError::NotFound)?;
                let connection = config
                    .connections
                    .get(self.text(1))
                    .ok_or(GatewayError::NotFound)?;
                let repositories: Vec<_> = self
                    .text(2)
                    .split(',')
                    .map(str::trim)
                    .map(str::to_owned)
                    .collect();
                for repository in &repositories {
                    if repository != "*" {
                        validate_repository(repository, connection.provider)?;
                    }
                }
                let operations: Vec<_> = self
                    .text(3)
                    .split(',')
                    .map(|value| match value.trim() {
                        "list-issues" => Ok(Operation::ListIssues),
                        "get-issue" => Ok(Operation::GetIssue),
                        "create-issue" => Ok(Operation::CreateIssue),
                        "api-read" => Ok(Operation::ApiRead),
                        "api-write" => Ok(Operation::ApiWrite),
                        _ => Err(GatewayError::InvalidRequest),
                    })
                    .collect::<Result<_>>()?;
                let ttl_minutes = if self.text(4).is_empty() {
                    "0"
                } else {
                    self.text(4)
                }
                .parse()
                .ok()
                .filter(|value| (0..=1440).contains(value))
                .ok_or(GatewayError::InvalidRequest)?;
                let requests_per_minute = self
                    .text(5)
                    .parse()
                    .ok()
                    .filter(|value| (1..=600).contains(value))
                    .ok_or(GatewayError::InvalidRequest)?;
                let options = GrantOptions {
                    name: self.text(0).to_owned(),
                    connection: self.text(1).to_owned(),
                    repositories,
                    operations,
                    ttl_minutes,
                    requests_per_minute,
                    max_calls: 0,
                    out: None,
                };
                Action::Grant {
                    options,
                    password: self.secret(6),
                }
            }
            Kind::Login => Action::Login {
                profile: WebDavProfile::new(self.text(0), self.text(1))?,
                password: self.secret(2),
            },
            Kind::OpenRemote(path) => Action::OpenRemote {
                path: path.clone(),
                password: self.secret(0),
            },
            Kind::Publish => Action::Publish {
                path: normalize_path(self.text(0))?,
                password: self.secret(1),
            },
            Kind::Unlock => Action::Unlock(self.secret(0)),
            Kind::Sync => Action::Sync(self.secret(0)),
            Kind::Revoke => {
                validate_name(self.text(0))?;
                Action::Revoke(self.text(0).to_owned())
            }
            Kind::AddSsh { generate } => {
                let category = if app.home { app.category.clone() } else { None };
                validate_title(self.text(0))?;
                if *generate {
                    validate_note(self.text(3))?;
                    Action::GenerateSsh {
                        category,
                        title: self.text(0).to_owned(),
                        algorithm: self.text(1).to_owned(),
                        comment: self.text(2).to_owned(),
                        note: self.text(3).to_owned(),
                        password: self.secret(4),
                    }
                } else {
                    validate_note(self.text(2))?;
                    Action::ImportSsh {
                        category,
                        title: self.text(0).to_owned(),
                        note: self.text(2).to_owned(),
                        material: self.secret(1),
                        password: self.secret(3),
                    }
                }
            }
            Kind::AddGpg => {
                validate_title(self.text(0))?;
                validate_note(self.text(2))?;
                Action::ImportGpg {
                    category: if app.home { app.category.clone() } else { None },
                    title: self.text(0).to_owned(),
                    note: self.text(2).to_owned(),
                    material: self.secret(1),
                    password: self.secret(3),
                }
            }
            Kind::EditKey {
                entry_id,
                login_type,
                ..
            } => {
                let ssh = login_type == crate::keys::payload::LOGIN_TYPE_SSH;
                validate_title(self.text(0))?;
                let note = if ssh { 2 } else { 1 };
                validate_note(self.text(note))?;
                Action::EditKey {
                    entry_id: entry_id.clone(),
                    title: self.text(0).to_owned(),
                    comment: ssh.then(|| self.text(1).to_owned()),
                    note: self.text(note).to_owned(),
                    password: self.secret(if ssh { 3 } else { 2 }),
                }
            }
        })
    }
}
