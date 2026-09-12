//! Browser geometry follows Yazi's root/tab/rail components: a path, three
//! proportional columns, and a status line. See docs/tui-design.md for sources.
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};

use super::browser::{Focus, grant_access, grant_status, quick_command};
use super::form::{Form, Input};
use super::{App, COMMANDS, Mode, Page, Popup, preview};
use crate::i18n::{Language, Message};
use crate::tr;

const BG: Color = Color::Rgb(40, 44, 52);
const PANEL: Color = Color::Rgb(33, 37, 43);
const FG: Color = Color::Rgb(171, 178, 191);
const LIGHT: Color = Color::Rgb(215, 218, 224);
pub(super) const DIM: Color = Color::Rgb(127, 132, 142);
pub(super) const ACCENT: Color = Color::Rgb(97, 175, 239);
pub(super) const CYAN: Color = Color::Rgb(86, 182, 194);
pub(super) const WARNING: Color = Color::Rgb(229, 192, 123);
pub(super) const GREEN: Color = Color::Rgb(152, 195, 121);
pub(super) const ERROR: Color = Color::Rgb(224, 108, 117);

#[derive(Clone, Copy)]
enum Icon {
    Folder,
    OpenFolder,
    Github,
    Gitlab,
    Grant,
    Database,
    File,
    Command,
    Cloud,
    Unlock,
}

impl Icon {
    fn text(self, nerd_font: bool) -> &'static str {
        match (self, nerd_font) {
            (Self::Folder, true) => "\u{f07b}",
            (Self::OpenFolder, true) => "\u{f07c}",
            (Self::Github, true) => "\u{f09b}",
            (Self::Gitlab, true) => "\u{f296}",
            (Self::Grant, true) => "\u{f084}",
            (Self::Database, true) => "\u{f1c0}",
            (Self::File, true) => "\u{f15b}",
            (Self::Command, true) => "\u{f120}",
            (Self::Cloud, true) => "\u{f0c2}",
            (Self::Unlock, true) => "\u{f09c}",
            (Self::Folder, false) => "+",
            (Self::OpenFolder, false) => "-",
            (Self::Github, false) => "G",
            (Self::Gitlab, false) => "L",
            (Self::Grant, false) => "*",
            (Self::Database, false) => "#",
            (Self::File, false) => "-",
            (Self::Command, false) => ":",
            (Self::Cloud, false) => "~",
            (Self::Unlock, false) => "o",
        }
    }
}

struct Record {
    name: String,
    suffix: String,
    icon: Icon,
    color: Color,
}

pub(super) fn clean(value: &str) -> String {
    value.chars().filter(|ch| {
        (*ch == '\n' || !ch.is_control())
            && !matches!(*ch, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    }).collect()
}

// Clip terminal cells, not bytes or characters; never split a CJK/emoji grapheme.
fn clipped(value: &str, width: usize, from_left: bool) -> String {
    let value = clean(value).replace('\n', " ");
    let line = Line::raw(value.as_str());
    if line.width() <= width {
        return value;
    }
    if width == 0 {
        return String::new();
    }
    let mut graphemes: Vec<_> = line.styled_graphemes(Style::default()).collect();
    if from_left {
        graphemes.reverse();
    }
    let mut kept = Vec::new();
    let mut used = 0;
    for grapheme in graphemes {
        let size = Line::raw(grapheme.symbol).width();
        if used + size >= width {
            break;
        }
        kept.push(grapheme.symbol);
        used += size;
    }
    if from_left {
        kept.reverse();
        format!("…{}", kept.concat())
    } else {
        format!("{}…", kept.concat())
    }
}

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .title(title)
        .title_style(Style::default().fg(ACCENT).bold())
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .padding(Padding::horizontal(1))
        .style(Style::default().bg(BG).fg(FG))
}

fn clear_modal(frame: &mut Frame<'_>, area: Rect) {
    let buffer = frame.buffer_mut();
    let area = area.intersection(buffer.area);
    // Clear also the exposed half of any background grapheme crossing an
    // edge. Otherwise a wide glyph just left of the modal makes the terminal
    // skip its first border cell, even though that cell contains a corner.
    for y in area.top()..area.bottom() {
        let mut x = buffer.area.left();
        while x < area.right() {
            let cell = &buffer[(x, y)];
            let end = x
                .saturating_add((Line::raw(cell.symbol()).width() as u16).max(1))
                .min(buffer.area.right());
            if (x < area.left() && end > area.left()) || end > area.right() {
                let style = cell.style();
                for column in x..end {
                    buffer[(column, y)].set_symbol(" ").set_style(style);
                }
            }
            x = end;
        }
    }
    frame.render_widget(Clear, area);
}

pub(super) fn render(frame: &mut Frame<'_>, app: &mut App) {
    let lang = app.language;
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::default().bg(BG).fg(FG)), area);
    if area.width < 70 || area.height < 20 {
        frame.render_widget(
            Paragraph::new(tr!(lang, TerminalTooSmall))
                .style(Style::default().fg(CYAN))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    if app.home {
        super::home::render(frame, app);
        match &mut app.mode {
            Mode::Form(form) => render_form(
                frame,
                form,
                app.failed.then_some(app.message.as_str()),
                area,
                lang,
            ),
            Mode::Popup(popup) => render_popup(frame, popup, area, &mut app.page_size, lang),
            _ => {}
        }
        return;
    }
    let root = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .split(area);
    let columns = Layout::horizontal([
        Constraint::Ratio(1, 8),
        Constraint::Ratio(4, 8),
        Constraint::Ratio(3, 8),
    ])
    .split(root[1]);
    header(frame, app, root[0]);
    navigation(frame, app, columns[0]);
    let current = Rect {
        x: columns[1].x + 1,
        width: columns[1].width.saturating_sub(2),
        ..columns[1]
    };
    app.page_size = current.height.max(1) as usize;
    records(frame, app, current);
    details(frame, app, columns[2]);
    for x in [columns[1].x, columns[1].right() - 1] {
        frame.render_widget(
            Paragraph::new(vec![Line::raw("│"); root[1].height as usize])
                .style(Style::default().fg(FG)),
            Rect {
                x,
                width: 1,
                ..root[1]
            },
        );
    }
    footer(frame, app, root[2]);
    match &mut app.mode {
        Mode::Form(form) => render_form(
            frame,
            form,
            app.failed.then_some(app.message.as_str()),
            area,
            lang,
        ),
        Mode::Popup(popup) => render_popup(frame, popup, area, &mut app.page_size, lang),
        Mode::Normal | Mode::Command(_) | Mode::Filter(_) => {}
    }
}

fn header(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let lang = app.language;
    let section = match app.page {
        Page::Dashboard => "overview",
        Page::Connections => "connections",
        Page::Grants => "grants",
        Page::WebDav => "webdav",
        Page::Help => "commands",
    };
    let mut path = format!("monica://{section}");
    if app.page == Page::WebDav {
        path.push('/');
        path.push_str(&app.folder);
    }
    let query = &app.filters[app.page.index()];
    if !query.is_empty() {
        path.push_str(&tr!(lang, FilterPathSuffix, query = query.as_str()));
    }
    frame.render_widget(
        Paragraph::new(format!(
            " {}",
            clipped(&path, area.width.saturating_sub(1) as usize, true)
        ))
        .style(Style::default().fg(CYAN).bold()),
        area,
    );
}

fn navigation(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let lang = app.language;
    for page in Page::ALL {
        let label = if Line::raw(page.label(lang)).width() <= area.width.saturating_sub(4) as usize
        {
            page.label(lang)
        } else {
            page.short_label(lang)
        };
        let selected = app.page == page;
        draw_record(
            frame,
            Rect {
                y: area.y + page.index() as u16,
                height: 1,
                ..area
            },
            &Record {
                name: label.to_owned(),
                suffix: String::new(),
                icon: if selected {
                    Icon::OpenFolder
                } else {
                    Icon::Folder
                },
                color: ACCENT,
            },
            selected,
            app.nerd_font,
        );
    }
}

fn record(app: &App, index: usize) -> Option<Record> {
    let lang = app.language;
    let (name, suffix, icon, color) = match app.page {
        Page::Connections => {
            let (name, connection) = app.config.as_ref()?.connections.iter().nth(index)?;
            (
                name.clone(),
                connection.provider.prefix().to_owned(),
                if connection.provider == crate::model::Provider::Github {
                    Icon::Github
                } else {
                    Icon::Gitlab
                },
                ACCENT,
            )
        }
        Page::Grants => {
            let grant = app.config.as_ref()?.grants.get(index)?;
            let active = grant_status(grant) == Message::GrantActive;
            (
                grant.name.clone(),
                if active {
                    lang.text(grant_access(grant))
                } else {
                    lang.text(grant_status(grant))
                }
                .to_owned(),
                Icon::Grant,
                if active { ACCENT } else { DIM },
            )
        }
        Page::WebDav => {
            let entry = app.entries.get(index)?;
            let mdbx = entry.path.to_ascii_lowercase().ends_with(".mdbx");
            (
                format!(
                    "{}{}",
                    entry.name(),
                    if entry.is_directory { "/" } else { "" }
                ),
                if entry.is_directory {
                    String::new()
                } else {
                    entry.size.map_or_else(String::new, human_size)
                },
                if entry.is_directory {
                    Icon::Folder
                } else if mdbx {
                    Icon::Database
                } else {
                    Icon::File
                },
                if entry.is_directory {
                    ACCENT
                } else if mdbx {
                    GREEN
                } else {
                    FG
                },
            )
        }
        Page::Dashboard => {
            let command = quick_command(index)?;
            let (label, icon) = match command.command {
                "add" => (tr!(lang, QuickAddAction), Icon::Github),
                "open" => (tr!(lang, OpenLocalAction), Icon::Database),
                "login" => (tr!(lang, LoginAction), Icon::Cloud),
                "grant" => (tr!(lang, CreateGrantTitle), Icon::Grant),
                _ => (tr!(lang, UnlockAction), Icon::Unlock),
            };
            (label.to_owned(), command.key.to_owned(), icon, ACCENT)
        }
        Page::Help => {
            let command = COMMANDS.get(index)?;
            (
                format!(":{}", command.command),
                command.key.to_owned(),
                Icon::Command,
                ACCENT,
            )
        }
    };
    Some(Record {
        name,
        suffix,
        icon,
        color,
    })
}

fn draw_record(
    frame: &mut Frame<'_>,
    area: Rect,
    record: &Record,
    selected: bool,
    nerd_font: bool,
) {
    let base = if selected {
        Style::default().bg(ACCENT).fg(BG).bold()
    } else {
        Style::default().bg(BG).fg(record.color)
    };
    let capacity = area.width.saturating_sub(4) as usize;
    let suffix = clean(&record.suffix);
    let suffix_width = Line::raw(suffix.as_str()).width();
    let suffix = if suffix_width + 8 <= capacity {
        suffix
    } else {
        String::new()
    };
    let reserved = if suffix.is_empty() {
        0
    } else {
        suffix_width + 1
    };
    let name = clipped(&record.name, capacity.saturating_sub(reserved), false);
    let padding = capacity
        .saturating_sub(Line::raw(name.as_str()).width() + Line::raw(suffix.as_str()).width());
    let cap_style = Style::default().fg(ACCENT).bg(BG);
    let (left, right) = if selected && nerd_font {
        ("\u{e0b6}", "\u{e0b4}")
    } else if selected {
        (">", " ")
    } else {
        (" ", " ")
    };
    let icon_color = if matches!(record.icon, Icon::Gitlab) {
        WARNING
    } else {
        record.color
    };
    let line = Line::from(vec![
        Span::styled(
            left,
            if selected && nerd_font {
                cap_style
            } else {
                base
            },
        ),
        Span::styled(
            record.icon.text(nerd_font),
            if selected { base } else { base.fg(icon_color) },
        ),
        Span::raw(" "),
        Span::raw(name),
        Span::raw(" ".repeat(padding)),
        Span::styled(suffix, if selected { base } else { base.fg(DIM) }),
        Span::styled(
            right,
            if selected && nerd_font {
                cap_style
            } else {
                base
            },
        ),
    ]);
    frame.render_widget(Paragraph::new(line).style(base), area);
}

fn records(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let lang = app.language;
    let visible = app.visible_rows(app.page);
    if visible.is_empty() {
        app.list_offsets[app.page.index()] = 0;
        let message = if !app.filters[app.page.index()].is_empty() {
            tr!(lang, NoMatchesList)
        } else {
            match app.page {
                Page::Connections => tr!(lang, NoConnectionsList),
                Page::Grants => tr!(lang, NoGrantsList),
                Page::WebDav if app.webdav.is_some() => tr!(lang, EmptyFolderList),
                Page::WebDav => tr!(lang, SignedOutList),
                _ => tr!(lang, NothingToShow),
            }
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::default().fg(DIM))
                .wrap(Wrap { trim: false }),
            Rect {
                x: area.x + 1,
                width: area.width.saturating_sub(2),
                ..area
            },
        );
        return;
    }
    let selected = app.selected[app.page.index()];
    let height = area.height as usize;
    // Keep the viewport stable until the cursor enters either scroll margin.
    let margin = 5.min(height / 2);
    let offset = &mut app.list_offsets[app.page.index()];
    if selected < offset.saturating_add(margin) {
        *offset = selected.saturating_sub(margin);
    } else if selected >= offset.saturating_add(height.saturating_sub(margin)) {
        *offset = selected.saturating_add(margin + 1).saturating_sub(height);
    }
    *offset = (*offset).min(visible.len().saturating_sub(height));
    let start = *offset;
    for (row, (visible_index, index)) in visible
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .enumerate()
    {
        if let Some(record) = record(app, *index) {
            draw_record(
                frame,
                Rect {
                    y: area.y + row as u16,
                    height: 1,
                    ..area
                },
                &record,
                visible_index == selected,
                app.nerd_font,
            );
        }
    }
}

// Wrap before scrolling so the final lines of long CJK notes and paths remain reachable.
fn wrapped_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut output = Vec::new();
    for line in lines {
        let mut spans = Vec::new();
        let mut used = 0;
        for grapheme in line.styled_graphemes(Style::default()) {
            let size = Line::raw(grapheme.symbol).width();
            if used + size > width as usize && !spans.is_empty() {
                let word = |symbol: &str| {
                    symbol
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                };
                let carry_start = if spans.len() > 1
                    && matches!(
                        grapheme.symbol,
                        "。" | "，"
                            | "、"
                            | "；"
                            | "："
                            | "！"
                            | "？"
                            | "）"
                            | "》"
                            | "」"
                            | "』"
                            | "】"
                    ) {
                    Some(spans.len() - 1)
                } else if word(grapheme.symbol) {
                    spans
                        .iter()
                        .rposition(|span: &Span<'_>| !word(&span.content))
                        .map(|index| index + 1)
                        .filter(|index| *index < spans.len())
                } else {
                    None
                };
                let carry = carry_start.map_or_else(Vec::new, |index| spans.split_off(index));
                output.push(Line::from(std::mem::take(&mut spans)));
                used = carry.iter().map(Span::width).sum();
                spans = carry;
            }
            spans.push(Span::styled(grapheme.symbol.to_owned(), grapheme.style));
            used += size;
        }
        output.push(Line::from(spans));
    }
    output
}

fn details(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    let inner = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };
    let lines = wrapped_lines(preview::content(app), inner.width);
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

fn broker_status(app: &App) -> (String, Color) {
    let lang = app.language;
    if let Some(broker) = &app.broker {
        let seconds = broker.remaining_seconds();
        (
            format!(
                "{} {:02}:{:02}",
                tr!(lang, StateOpen),
                seconds / 60,
                seconds % 60
            ),
            GREEN,
        )
    } else if app.external_busy {
        (tr!(lang, StateBusy).to_owned(), WARNING)
    } else {
        (tr!(lang, StateLocked).to_owned(), WARNING)
    }
}

fn capsule_edge(text: &'static str, color: Color, background: Color) -> Span<'static> {
    Span::styled(text, Style::default().fg(color).bg(background))
}

fn footer(frame: &mut Frame<'_>, app: &App, area: Rect) {
    let lang = app.language;
    let (open, close) = if app.nerd_font {
        ("\u{e0b6}", "\u{e0b4}")
    } else {
        (" ", " ")
    };
    let mode = match &app.mode {
        Mode::Normal => match app.focus {
            Focus::Navigation => tr!(lang, ModeNavigation),
            Focus::List => tr!(lang, ModeNormal),
            Focus::Preview => tr!(lang, ModePreview),
        },
        Mode::Command(_) => tr!(lang, ModeCommand),
        Mode::Filter(_) => tr!(lang, ModeFilter),
        Mode::Form(form) if form.insert => tr!(lang, ModeInsert),
        Mode::Form(_) => tr!(lang, ModeForm),
        Mode::Popup(_) => tr!(lang, ModePopup),
    };
    let main = Style::default().fg(BG).bg(ACCENT).bold();
    let alternate = Style::default().fg(ACCENT).bg(LIGHT).bold();
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
    let selected = app
        .selected_index(app.page)
        .and_then(|index| record(app, index));
    let kind = match app.page {
        Page::Dashboard => tr!(lang, KindStart).to_owned(),
        Page::Connections => selected
            .as_ref()
            .map_or(String::new(), |record| record.suffix.to_uppercase()),
        Page::Grants => tr!(lang, KindGrant).to_owned(),
        Page::WebDav => app
            .selected_index(Page::WebDav)
            .and_then(|index| app.entries.get(index))
            .map_or("DAV", |entry| {
                if entry.is_directory {
                    tr!(lang, KindDirectory)
                } else if entry.path.to_ascii_lowercase().ends_with(".mdbx") {
                    "MDBX"
                } else {
                    tr!(lang, KindFile)
                }
            })
            .to_owned(),
        Page::Help => tr!(lang, ModeCommand).to_owned(),
    };
    left.extend([
        capsule_edge(close, ACCENT, LIGHT),
        Span::styled(format!(" {kind} "), alternate),
        capsule_edge(close, LIGHT, BG),
    ]);
    let count = app.rows();
    let index = if count == 0 {
        0
    } else {
        app.selected[app.page.index()] + 1
    };
    let percent = if app.focus == Focus::Preview {
        position(app.preview_scroll, app.preview_max_scroll, lang)
    } else {
        position(index.saturating_sub(1), count.saturating_sub(1), lang)
    };
    let (broker, color) = broker_status(app);
    let right = Line::from(vec![
        Span::styled(
            format!(" F2:{} ? ", if lang == Language::En { "EN" } else { "中" }),
            Style::default().fg(DIM),
        ),
        Span::styled(format!(" {broker} "), Style::default().fg(color)),
        capsule_edge(open, LIGHT, BG),
        Span::styled(format!(" {percent} "), alternate),
        capsule_edge(open, ACCENT, LIGHT),
        Span::styled(format!(" {index}/{count} "), main),
        capsule_edge(close, ACCENT, BG),
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
        (
            format!(
                "{}{}",
                lang.text(app.pending_label),
                if app.quitting {
                    tr!(lang, QuitWhenDone)
                } else {
                    ""
                }
            ),
            WARNING,
        )
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
            selected.map_or_else(|| tr!(lang, AddHelpFooter).to_owned(), |record| record.name),
            LIGHT,
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

fn position(index: usize, last: usize, lang: Language) -> String {
    if index == 0 {
        tr!(lang, PositionTop).to_owned()
    } else if index >= last {
        tr!(lang, PositionBottom).to_owned()
    } else {
        format!("{}%", index * 100 / last.max(1))
    }
}

fn render_input(frame: &mut Frame<'_>, input: &Input, prefix: &str, area: Rect) {
    let cursor = prefix.len() + Line::raw(clean(&input.value[..input.cursor])).width();
    let scroll = cursor.saturating_sub(area.width.saturating_sub(1) as usize);
    frame.render_widget(
        Paragraph::new(format!("{prefix}{}", clean(&input.value)))
            .scroll((0, scroll as u16))
            .style(Style::default().fg(CYAN).bg(BG)),
        area,
    );
    frame.set_cursor_position((area.x + cursor.saturating_sub(scroll) as u16, area.y));
}

fn render_popup(
    frame: &mut Frame<'_>,
    popup: &mut Popup,
    screen: Rect,
    page_size: &mut usize,
    lang: Language,
) {
    let width = screen.width.saturating_sub(6).min(104);
    let lines = wrapped_lines(
        popup
            .lines
            .iter()
            .map(|line| Line::raw(clean(line)))
            .collect(),
        width.saturating_sub(4),
    );
    let height = (lines.len() as u16 + 2)
        .min(screen.height.saturating_sub(4))
        .max(5);
    let area = centered(screen, width, height);
    clear_modal(frame, area);
    let title = format!(" {} ", clean(&popup.title));
    let block = panel(&title).title_bottom(Line::raw(tr!(lang, PopupFooter)).right_aligned());
    let inner = block.inner(area);
    popup.max_scroll = lines.len().saturating_sub(inner.height as usize);
    popup.scroll = popup.scroll.min(popup.max_scroll);
    *page_size = inner.height.max(1) as usize;
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(lines).scroll((popup.scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
}

fn render_form(
    frame: &mut Frame<'_>,
    form: &Form,
    error: Option<&str>,
    screen: Rect,
    lang: Language,
) {
    let width = screen.width.saturating_sub(6).min(90);
    let notice = wrapped_lines(
        vec![Line::raw(clean(&form.notice))],
        width.saturating_sub(4),
    );
    let notice_height = (notice.len() as u16).min(3);
    let height =
        (form.fields.len() as u16 * 3 + notice_height + 5).min(screen.height.saturating_sub(3));
    let area = centered(screen, width, height);
    clear_modal(frame, area);
    let title = format!(" {} ", form.title);
    let block = panel(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let split = Layout::vertical([
        Constraint::Length(notice_height + 1),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .split(inner);
    frame.render_widget(
        Paragraph::new(notice).style(Style::default().fg(DIM)),
        split[0],
    );
    let visible = (split[1].height / 3).max(1) as usize;
    let start = form.selected.saturating_sub(visible.saturating_sub(1));
    for (row, (index, field)) in form
        .fields
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .enumerate()
    {
        let field_area = Rect {
            y: split[1].y + row as u16 * 3,
            height: 2,
            ..split[1]
        };
        let active = index == form.selected;
        let label = format!(
            "{} {}{}",
            if active { "›" } else { " " },
            field.label,
            if field.secret {
                tr!(lang, HiddenSuffix)
            } else {
                ""
            }
        );
        frame.render_widget(
            Paragraph::new(label).style(Style::default().fg(if active { ACCENT } else { DIM })),
            Rect {
                height: 1,
                ..field_area
            },
        );
        let value = clean(&field.display());
        let prefix_width = if field.secret {
            field.input.value[..field.input.cursor].chars().count()
        } else {
            Line::raw(&field.input.value[..field.input.cursor]).width()
        };
        let scroll =
            prefix_width.saturating_sub(field_area.width.saturating_sub(1) as usize) as u16;
        let text = if value.is_empty() && active {
            field.hint.to_owned()
        } else {
            value
        };
        frame.render_widget(
            Paragraph::new(text).scroll((0, scroll)).style(
                Style::default()
                    .fg(if field.input.value.is_empty() {
                        DIM
                    } else {
                        LIGHT
                    })
                    .bg(if active { PANEL } else { BG }),
            ),
            Rect {
                y: field_area.y + 1,
                height: 1,
                ..field_area
            },
        );
        if active && form.insert {
            frame.set_cursor_position((
                field_area.x
                    + (prefix_width as u16)
                        .saturating_sub(scroll)
                        .min(field_area.width.saturating_sub(1)),
                field_area.y + 1,
            ));
        }
    }
    let hint = tr!(
        lang,
        FormFooter,
        mode = if form.insert {
            tr!(lang, ModeInsert)
        } else {
            tr!(lang, ModeNormal)
        },
        index = form.selected + 1,
        count = form.fields.len()
    );
    let note = error.map_or_else(
        || {
            form.fields
                .get(form.selected)
                .map_or_else(String::new, |field| {
                    clipped(field.hint, split[2].width as usize, false)
                })
        },
        |error| clipped(error, split[2].width as usize, false),
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(hint),
            Line::styled(
                note,
                Style::default().fg(if error.is_some() { ERROR } else { DIM }),
            ),
        ])
        .style(Style::default().fg(DIM)),
        split[2],
    );
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub(super) fn timestamp(value: i64) -> String {
    chrono::DateTime::from_timestamp(value, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "—".to_owned())
}

pub(super) fn human_size(size: u64) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MiB", size as f64 / (1024.0 * 1024.0))
    } else if size >= 1024 {
        format!("{:.1} KiB", size as f64 / 1024.0)
    } else {
        format!("{size} B")
    }
}
