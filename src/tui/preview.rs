//! Public metadata only. No vault, client-file, filesystem or network reads.
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::browser::{grant_access, grant_status, quick_command};
use super::view::{ACCENT, DIM, ERROR, GREEN, WARNING, clean, human_size, timestamp};
use super::{App, COMMANDS, Page};
use crate::config::Grant;
use crate::i18n::{Language, Message};
use crate::model::Provider;
use crate::tr;

fn heading(text: impl AsRef<str>) -> Line<'static> {
    Line::styled(clean(text.as_ref()), Style::default().fg(ACCENT).bold())
}

fn label(text: impl AsRef<str>) -> Line<'static> {
    Line::styled(clean(text.as_ref()), Style::default().fg(DIM))
}

fn field(lines: &mut Vec<Line<'static>>, name: &str, value: impl AsRef<str>) {
    let prefix = format!("{name}  ");
    let indent = " ".repeat(Line::raw(prefix.as_str()).width());
    for (index, value) in clean(value.as_ref()).lines().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(
                if index == 0 {
                    prefix.clone()
                } else {
                    indent.clone()
                },
                Style::default().fg(DIM),
            ),
            Span::raw(value.to_owned()),
        ]));
    }
}

fn note(lines: &mut Vec<Line<'static>>, text: &str, lang: Language) {
    lines.push(Line::default());
    lines.push(label(tr!(lang, PurposeHeading)));
    lines.push(Line::raw(clean(if text.is_empty() {
        tr!(lang, NoPurpose)
    } else {
        text
    })));
}

fn tools(lines: &mut Vec<Line<'static>>, grant: &Grant, provider: Provider, lang: Language) {
    lines.push(label(tr!(lang, ShortCommands)));
    lines.extend(
        grant
            .operations
            .iter()
            .map(|operation| Line::raw(format!("  {}", operation.tool_name(provider)))),
    );
}

pub(super) fn content(app: &App) -> Vec<Line<'static>> {
    let lang = app.language;
    if app.rows() == 0 && !app.filters[app.page.index()].is_empty() {
        return vec![
            heading(tr!(lang, NoMatchesHeading)),
            Line::raw(tr!(lang, ChangeFilterHint)),
        ];
    }
    match app.page {
        Page::Dashboard => dashboard(app),
        Page::Connections => connection(app),
        Page::Grants => grant(app),
        Page::WebDav => webdav(app),
        Page::Help => command(app),
    }
}

fn connection(app: &App) -> Vec<Line<'static>> {
    let lang = app.language;
    let Some(name) = app.selected_connection() else {
        return vec![
            heading(tr!(lang, FirstConnectionHeading)),
            Line::default(),
            Line::raw(tr!(lang, FirstConnectionInput)),
            Line::raw(tr!(lang, FirstConnectionGrant)),
            Line::default(),
            Line::raw(tr!(lang, ExistingVaultHint)),
        ];
    };
    let Some(config) = &app.config else {
        return Vec::new();
    };
    let Some(connection) = config.connections.get(&name) else {
        return Vec::new();
    };
    let grants: Vec<_> = config
        .grants
        .iter()
        .filter(|grant| grant.connection == name)
        .collect();
    let mut lines = vec![
        heading(&name),
        label(tr!(
            lang,
            ProviderGrants,
            provider = connection.provider.prefix(),
            count = grants.len()
        )),
    ];
    note(&mut lines, &connection.note, lang);
    field(&mut lines, "connection", &name);
    if grants.is_empty() {
        lines.push(Line::default());
        lines.push(label(tr!(lang, PageGrants)));
        lines.push(Line::raw(tr!(lang, NoGrantsHint)));
    } else {
        for grant in grants {
            lines.push(Line::default());
            lines.push(scope_title(grant, lang));
            field(
                &mut lines,
                tr!(lang, RepositoriesHeading),
                grant
                    .repositories
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            tools(&mut lines, grant, connection.provider, lang);
            field(
                &mut lines,
                tr!(lang, ExpiryHeading),
                if grant.expires_at == 0 {
                    if lang == Language::En {
                        "No expiry".to_owned()
                    } else {
                        "长期有效".to_owned()
                    }
                } else {
                    timestamp(grant.expires_at)
                },
            );
        }
    }
    lines.push(Line::default());
    field(&mut lines, tr!(lang, AddressHeading), &connection.api_base);
    lines
}

fn scope_title(grant: &Grant, lang: Language) -> Line<'static> {
    let status = lang.text(grant_status(grant));
    Line::from(vec![
        Span::styled(clean(&grant.name), Style::default().fg(ACCENT)),
        Span::styled(
            format!(" · {}", lang.text(grant_access(grant))),
            Style::default().fg(if grant_access(grant) == Message::AccessReadOnly {
                GREEN
            } else {
                WARNING
            }),
        ),
        Span::styled(
            format!(" · {status}"),
            Style::default().fg(if grant_status(grant) == Message::GrantActive {
                GREEN
            } else {
                ERROR
            }),
        ),
    ])
}

fn grant(app: &App) -> Vec<Line<'static>> {
    let lang = app.language;
    let Some(grant) = app.selected_grant() else {
        return vec![
            heading(tr!(lang, ChoosePermissionsHeading)),
            Line::default(),
            Line::raw(tr!(lang, ChoosePermissionsHint)),
            Line::raw(tr!(lang, ExplicitWriteHint)),
            Line::default(),
            Line::raw(tr!(lang, QuickAddHint)),
        ];
    };
    let mut lines = vec![scope_title(grant, lang)];
    field(&mut lines, "connection", &grant.connection);
    let connection = app
        .config
        .as_ref()
        .and_then(|config| config.connections.get(&grant.connection));
    if let Some(connection) = connection {
        note(&mut lines, &connection.note, lang);
    }
    lines.push(Line::default());
    field(
        &mut lines,
        tr!(lang, RepositoriesHeading),
        grant
            .repositories
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n"),
    );
    if let Some(connection) = connection {
        tools(&mut lines, grant, connection.provider, lang);
    }
    field(
        &mut lines,
        tr!(lang, ExpiryHeading),
        if grant.expires_at == 0 {
            if lang == Language::En {
                "No expiry".to_owned()
            } else {
                "长期有效".to_owned()
            }
        } else {
            timestamp(grant.expires_at)
        },
    );
    field(
        &mut lines,
        tr!(lang, RateLimitHeading),
        tr!(lang, RequestsPerMinute, count = grant.requests_per_minute),
    );
    if let Some(path) = &grant.client_file {
        lines.push(Line::default());
        lines.push(label(tr!(lang, McpSettingsHeading)));
        lines.push(Line::raw(clean(
            &path.with_extension("mcp.json").display().to_string(),
        )));
    }
    lines
}

fn webdav(app: &App) -> Vec<Line<'static>> {
    let lang = app.language;
    let entry = app
        .selected_index(Page::WebDav)
        .and_then(|index| app.entries.get(index));
    let mut lines = if let Some(entry) = entry {
        let mut lines = vec![heading(entry.name())];
        if entry.is_directory {
            lines.push(label(tr!(lang, FolderActions)));
        } else {
            lines.push(label(format!(
                "{} · {}",
                if entry.path.to_ascii_lowercase().ends_with(".mdbx") {
                    "MDBX"
                } else {
                    tr!(lang, FileLabel)
                },
                entry
                    .size
                    .map_or_else(|| tr!(lang, UnknownSize).to_owned(), human_size)
            )));
        }
        lines.push(Line::default());
        field(
            &mut lines,
            tr!(lang, PathHeading),
            format!("/{}", entry.path),
        );
        if !entry.is_directory {
            field(
                &mut lines,
                tr!(lang, RevisionHeading),
                if entry.etag.is_some() {
                    tr!(lang, EtagAvailable)
                } else {
                    tr!(lang, EtagMissing)
                },
            );
            lines.push(Line::default());
            lines.push(Line::raw(
                if entry.path.to_ascii_lowercase().ends_with(".mdbx") {
                    tr!(lang, OpenRemoteHint)
                } else {
                    tr!(lang, ChooseMdbxHint)
                },
            ));
        }
        lines
    } else {
        vec![
            heading(if app.webdav.is_some() {
                tr!(lang, EmptyFolderHeading)
            } else {
                tr!(lang, ConnectWebDavHeading)
            }),
            Line::default(),
            Line::raw(if app.webdav.is_some() {
                tr!(lang, PublishRefreshHint)
            } else {
                tr!(lang, SignInBrowseHint)
            }),
        ]
    };
    if let Some(profile) = app
        .webdav
        .as_ref()
        .map(|client| &client.profile)
        .or(app.profile.as_ref())
    {
        lines.push(Line::default());
        field(
            &mut lines,
            if app.webdav.is_some() {
                tr!(lang, AccountHeading)
            } else {
                tr!(lang, SignedOutLabel)
            },
            &profile.username,
        );
        field(&mut lines, tr!(lang, AddressHeading), &profile.base_url);
    }
    if let Some(binding) = app
        .config
        .as_ref()
        .and_then(|config| config.webdav.as_ref())
    {
        lines.push(Line::default());
        field(&mut lines, tr!(lang, SyncHeading), &binding.path);
        if binding.etag.is_none() {
            lines.push(Line::styled(
                tr!(lang, EtagMissing),
                Style::default().fg(WARNING),
            ));
        }
        field(
            &mut lines,
            tr!(lang, TimeHeading),
            timestamp(binding.last_sync),
        );
    }
    lines.push(Line::default());
    lines.push(label(tr!(lang, WebDavSessionHint)));
    lines
}

fn dashboard(app: &App) -> Vec<Line<'static>> {
    let lang = app.language;
    let Some(command) = app.selected_index(Page::Dashboard).and_then(quick_command) else {
        return Vec::new();
    };
    let mut lines = vec![
        heading(format!(":{} · {}", command.command, command.key)),
        Line::raw(lang.text(command.description)),
    ];
    lines.push(Line::default());
    lines.push(label(tr!(lang, CliAlternativeHeading)));
    lines.push(Line::raw(lang.text(command.cli)));
    lines.push(Line::default());
    if let Some(config) = &app.config {
        field(
            &mut lines,
            tr!(lang, VaultHeading),
            config.vault.display().to_string(),
        );
        field(
            &mut lines,
            tr!(lang, ConfiguredHeading),
            tr!(
                lang,
                ConfiguredCounts,
                connections = config.connections.len(),
                grants = config.grants.len()
            ),
        );
        if let Some(remote) = &config.webdav {
            field(&mut lines, tr!(lang, SyncHeading), &remote.path);
            if remote.etag.is_none() {
                lines.push(Line::styled(
                    tr!(lang, EtagMissing),
                    Style::default().fg(WARNING),
                ));
            }
            field(
                &mut lines,
                tr!(lang, TimeHeading),
                timestamp(remote.last_sync),
            );
        }
    } else {
        lines.push(label(tr!(lang, FirstUseHeading)));
        lines.push(Line::raw(tr!(lang, FirstUseHint)));
    }
    lines.push(Line::default());
    lines.push(label(tr!(lang, AiVisibleHeading)));
    lines.push(Line::raw(tr!(lang, AiVisibleHint)));
    lines
}

fn command(app: &App) -> Vec<Line<'static>> {
    let lang = app.language;
    let Some(command) = app
        .selected_index(Page::Help)
        .and_then(|index| COMMANDS.get(index))
    else {
        return Vec::new();
    };
    let mut lines = vec![
        heading(format!(":{}", command.command)),
        Line::raw(lang.text(command.description)),
        Line::default(),
        label(tr!(lang, TerminalCommandHeading)),
        Line::raw(lang.text(command.cli)),
        Line::default(),
    ];
    field(
        &mut lines,
        tr!(lang, ExecuteHeading),
        if command.key.is_empty() {
            "Enter".to_owned()
        } else {
            tr!(lang, ExecuteKey, key = command.key)
        },
    );
    lines.push(Line::default());
    lines.push(label(tr!(lang, CommandSearchHint)));
    lines
}
