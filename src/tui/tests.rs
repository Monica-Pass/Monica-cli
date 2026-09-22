use super::*;
use home::public_field;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

/// Private material has no reason to be drawn anywhere on screen: forms mask it, and
/// previews read a summary that never carries it.
const KEY_MATERIAL: [&str; 3] = [
    "-----BEGIN OPENSSH PRIVATE KEY-----",
    "-----BEGIN RSA PRIVATE KEY-----",
    "-----BEGIN PGP PRIVATE KEY BLOCK-----",
];

#[test]
fn editing_defers_database_password_and_escape_preserves_draft() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::En,
    );
    app.show_form(Kind::Category(None));
    app.paste("Work projects");
    let edit = render(&mut app, 70, 20);
    assert!(edit.contains("Work projects"));
    assert!(!edit.contains("Master password"));
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(app.pending.is_none());
    assert!(render(&mut app, 70, 20).contains("Unlock to save"));
    app.paste("synthetic-password");
    assert!(!render(&mut app, 70, 20).contains("synthetic-password"));
    app.key(key(KeyCode::F(2)));
    assert!(matches!(&app.mode, Mode::Form(form) if form.authenticating));
    app.key(key(KeyCode::Esc));
    let Mode::Form(form) = &app.mode else {
        panic!("draft lost")
    };
    assert!(!form.authenticating);
    assert_eq!(form.fields[0].input.value.as_str(), "Work projects");
    assert!(form.fields[1].input.value.is_empty());
    assert!(app.pending.is_none());
}

#[test]
fn database_rail_moves_and_targets_the_selected_database() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::ZhCn,
    );
    app.databases = vec![
        crate::databases::Database {
            id: "current".into(),
            name: "个人".into(),
            path: directory.path().join("personal.mdbx"),
            current: true,
        },
        crate::databases::Database {
            id: "saved-id".into(),
            name: "工作".into(),
            path: directory.path().join("work.mdbx"),
            current: false,
        },
    ];
    app.key(key(KeyCode::Char('d')));
    assert_eq!(app.focus, Focus::Navigation);
    app.key(key(KeyCode::Down));
    let screen = render(&mut app, 100, 30).replace(' ', "");
    assert!(screen.contains("个人") && screen.contains("工作"));
    app.key(key(KeyCode::Enter));
    assert!(
        matches!(&app.mode, Mode::Form(Form { kind: Kind::SwitchDatabase { id, name }, .. }) if id == "saved-id" && name == "工作")
    );
    app.mode = Mode::Normal;
    app.databases.clear();
    app.key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn home_browses_categories_and_entries_as_one_tree() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::ZhCn,
    );
    assert!(app.home);
    app.apply(browsed(crate::library::Library {
        categories: vec![
            crate::library::Category {
                id: "root".into(),
                parent: None,
                title: "工作".into(),
            },
            crate::library::Category {
                id: "child".into(),
                parent: Some("root".into()),
                title: "项目".into(),
            },
        ],
        entries: vec![crate::library::Entry {
            id: "token".into(),
            category: "child".into(),
            title: "GitLab".into(),
            kind: "api-token".into(),
        }],
    }));
    // A category is the header of its own subtree, so the entry below it is
    // reachable without opening the folder first.
    let ids: Vec<String> = app
        .home_rows()
        .iter()
        .map(|row| row.id().to_owned())
        .collect();
    assert_eq!(ids, ["root", "child", "token"]);
    app.key(key(KeyCode::Enter));
    assert_eq!(app.category.as_deref(), Some("root"));
    app.key(key(KeyCode::Enter));
    assert_eq!(app.category.as_deref(), Some("child"));
    assert_eq!(app.selected_home_row().unwrap().id(), "token");
    for (width, height) in [(70, 20), (100, 30), (150, 40)] {
        let screen = render(&mut app, width, height);
        assert!(screen.contains("GitLab"), "{width}x{height}");
        assert!(screen.contains("api-token"), "{width}x{height}");
        assert!(!screen.contains("create-issue"));
        capture_buffer(
            &format!("home-{width}x{height}"),
            &draw(&mut app, width, height),
        );
    }
    app.key(key(KeyCode::Char('h')));
    assert_eq!(app.category.as_deref(), Some("root"));
    app.key(key(KeyCode::Tab));
    assert_eq!(app.focus, Focus::Preview);
    app.key(key(KeyCode::Char('h')));
    assert_eq!(app.focus, Focus::List);
    app.key(key(KeyCode::Char('e')));
    assert!(
        matches!(&app.mode, Mode::Form(Form { kind: Kind::RenameCategory { id, .. }, .. }) if id == "child")
    );
    app.mode = Mode::Normal;
    app.key(key(KeyCode::Char('n')));
    assert!(
        matches!(&app.mode, Mode::Form(Form { kind: Kind::Category(parent), .. })
            if parent.as_deref() == Some("child"))
    );
    app.mode = Mode::Normal;
    app.key(key(KeyCode::F(3)));
    assert!(!app.home);
    app.key(key(KeyCode::F(3)));
    assert!(app.home);
    let mut updated = app.library.clone().unwrap();
    updated.categories[1].title = "新分类名".into();
    updated.categories.reverse();
    app.home_restore = Some((Some("child".into()), Some("token".into())));
    app.apply(browsed(updated));
    assert_eq!(app.category.as_deref(), Some("child"));
    assert_eq!(app.selected_home_row().unwrap().id(), "token");
}

#[test]
fn a_row_only_deletes_after_its_own_name_is_typed_back() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::En,
    );
    let bound = "11111111-1111-1111-1111-111111111111".to_owned();
    let mut config = Config::new(directory.path().join("synthetic.mdbx"));
    config.connections.insert(
        "work-gitlab".to_owned(),
        crate::config::Connection {
            provider: crate::model::Provider::Gitlab,
            credential_id: bound.clone(),
            api_base: "https://gitlab.example.com/".to_owned(),
            note: String::new(),
        },
    );
    app.apply(browsed(crate::library::Library {
        categories: vec![
            crate::library::Category {
                id: "work".into(),
                parent: None,
                title: "Work".into(),
            },
            crate::library::Category {
                id: "archive".into(),
                parent: None,
                title: "Archive".into(),
            },
        ],
        entries: vec![
            crate::library::Entry {
                id: bound.clone(),
                category: "work".into(),
                title: "GitLab".into(),
                kind: "api-token".into(),
            },
            crate::library::Entry {
                id: "scratch".into(),
                category: "work".into(),
                title: "Scratch".into(),
                kind: "login".into(),
            },
        ],
    }));
    app.config = Some(config);
    assert_eq!(
        app.home_rows().iter().map(|r| r.id()).collect::<Vec<_>>(),
        [
            "action:grants",
            "work",
            bound.as_str(),
            "scratch",
            "archive"
        ]
    );
    app.key(key(KeyCode::Down));
    assert_eq!(app.selected_home_row().unwrap().id(), "work");
    // The keybar advertises the binding on a row that can use it.
    let footer = text(&mut app, 120, 24)
        .lines()
        .last()
        .unwrap_or_default()
        .to_owned();
    assert!(footer.contains("D delete"), "{footer}");
    // Contents are never taken down with their category, so it is refused on the spot.
    app.key(key(KeyCode::Char('D')));
    assert!(matches!(app.mode, Mode::Normal));
    assert!(app.pending.is_none());
    assert!(
        app.message.starts_with("'Work' still holds 2 entry(s)"),
        "{}",
        app.message
    );

    // A credential behind a saved connection is deleted as that connection.
    app.key(key(KeyCode::Down));
    assert_eq!(app.selected_home_row().unwrap().id(), bound);
    app.key(key(KeyCode::Char('D')));
    let Mode::Form(form) = &app.mode else {
        panic!("delete form never opened")
    };
    assert_eq!(form.title, "Delete");
    assert!(form.notice.contains("GitLab"));
    assert!(form.fields[0].input.value.is_empty());
    app.paste("Gitlab");
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(
        matches!(&app.mode, Mode::Form(form) if matches!(form.kind, Kind::Delete { .. })),
        "a mistyped name left the form"
    );
    assert!(app.pending.is_none());
    assert!(app.message.contains("did not match 'GitLab'"));

    let delete = |app: &mut App| -> actions::DeleteTarget {
        app.mode = Mode::Normal;
        app.key(key(KeyCode::Char('D')));
        let Mode::Form(mut form) = std::mem::replace(&mut app.mode, Mode::Normal) else {
            panic!("delete form never opened")
        };
        let Kind::Delete { expect, .. } = form.kind.clone() else {
            panic!("delete opened another form")
        };
        assert!(form.fields[0].input.value.is_empty());
        form.fields[0].input = Input::new(&expect, 4096);
        form.fields[1].input = Input::new("synthetic-password", 4096);
        let Action::Delete { target, password } = form.action(app).unwrap() else {
            panic!("delete form produced no action")
        };
        assert_eq!(password.as_str(), "synthetic-password");
        target
    };
    let target = delete(&mut app);
    assert!(
        matches!(target, actions::DeleteTarget::Connection(name) if name == "work-gitlab"),
        "a bound credential must remove its connection and grants"
    );
    app.mode = Mode::Normal;
    app.key(key(KeyCode::Down));
    assert_eq!(app.selected_home_row().unwrap().id(), "scratch");
    assert!(matches!(
        delete(&mut app),
        actions::DeleteTarget::Entry(id) if id == "scratch"
    ));
    app.mode = Mode::Normal;
    app.key(key(KeyCode::Down));
    assert_eq!(app.selected_home_row().unwrap().id(), "archive");
    assert!(matches!(
        delete(&mut app),
        actions::DeleteTarget::Category(id) if id == "archive"
    ));
}

#[test]
fn rails_stay_on_the_layout_columns_across_widths_languages_and_panes() {
    for language in [Language::En, Language::ZhCn] {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new(
            ConfigStore::new(directory.path().join("gateway.json")),
            language,
        );
        app.nerd_font = true;
        app.databases = crowded_databases(directory.path());
        app.apply(browsed(crowded_library()));
        app.databases = crowded_databases(directory.path());
        let other = tempfile::tempdir().unwrap();
        let mut browser = manager_fixture(other.path());
        browser.language = language;
        browser.databases = crowded_databases(other.path());
        for width in 70..=140 {
            let home = assert_rails(&mut app, width, 24);
            let manager = assert_rails(&mut browser, width, 24);
            assert_eq!(home, manager, "screens disagree on rails at {width}x24");
        }
        for (width, height) in [(69, 24), (70, 19)] {
            assert!(
                !render(&mut app, width, height).contains('│'),
                "{width}x{height}"
            );
        }
    }
}

#[test]
fn home_keybar_drops_hint_groups_whole_not_mid_binding() {
    for language in [Language::En, Language::ZhCn] {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new(
            ConfigStore::new(directory.path().join("gateway.json")),
            language,
        );
        app.nerd_font = true;
        app.databases = crowded_databases(directory.path());
        app.apply(browsed(crowded_library()));
        app.databases = crowded_databases(directory.path());
        let primary = match language {
            Language::En => "Enter open",
            _ => "Enter 进入",
        };
        for width in 70..=140 {
            let footer = text(&mut app, width, 24)
                .lines()
                .last()
                .unwrap_or_default()
                .to_owned();
            assert!(
                !footer.contains('…') && footer.contains(primary),
                "{language:?} {width}x24: {footer}"
            );
        }
    }
}

#[test]
fn home_lists_key_rows_and_previews_public_metadata_only() {
    for language in [Language::En, Language::ZhCn] {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new(
            ConfigStore::new(directory.path().join("gateway.json")),
            language,
        );
        app.nerd_font = true;
        app.databases = crowded_databases(directory.path());
        app.apply(browsed_keys(
            docs_library(),
            vec![ssh_summary(), gpg_summary()],
        ));
        app.databases = crowded_databases(directory.path());
        let rows = app.home_rows();
        // A key entry is an ordinary `login` row until the read says otherwise, so
        // it must appear exactly once and never as a second, token-shaped row.
        for id in ["aws", "netflix"] {
            assert_eq!(rows.iter().filter(|row| row.id() == id).count(), 1);
        }
        app.home_selected = rows.iter().position(|row| row.id() == "aws").unwrap();
        let ssh = text(&mut app, 120, 30);
        assert!(ssh.contains("ED25519 256"), "{ssh}");
        assert!(ssh.contains(language.text(Message::KeyFingerprintHeading)));
        assert!(ssh.contains("SHA256:sshmockfingerprint"), "{ssh}");
        assert!(ssh.contains("joey@work"), "{ssh}");
        assert!(ssh.contains("ssh-ed25519 AAAAMOCKPUBLIC"), "{ssh}");
        assert!(ssh.contains(language.text(Message::KeyPrivateHeading)));
        for banner in KEY_MATERIAL {
            assert!(!ssh.contains(banner), "key text leaked");
        }
        assert!(
            text(&mut app, 120, 30)
                .lines()
                .last()
                .unwrap_or_default()
                .contains(language.text(Message::KeysSshEntry)),
            "a key row must not offer the token bindings"
        );
        capture_buffer(
            &format!("home-{}-ssh", language.choice().code()),
            &draw(&mut app, 120, 30),
        );
        assert_rails(&mut app, 120, 30);
        app.home_selected = rows.iter().position(|row| row.id() == "netflix").unwrap();
        let gpg = text(&mut app, 120, 30);
        assert!(gpg.contains("RSA 3072"), "{gpg}");
        assert!(gpg.contains(language.text(Message::KeyUidHeading)), "{gpg}");
        assert!(gpg.contains("Joyin Joester"), "{gpg}");
        assert!(gpg.contains(language.text(Message::KeyChunksHeading)));
        // A public ring still has no private half, and the preview says so.
        assert!(gpg.contains(language.text(Message::KeyValueNo)), "{gpg}");
        // The keybar names the field y copies, and a GPG row has no public key line.
        assert!(gpg.contains(language.text(Message::KeysGpgEntry)), "{gpg}");
        assert!(!gpg.contains(language.text(Message::KeysSshEntry)), "{gpg}");
        capture_buffer(
            &format!("home-{}-gpg", language.choice().code()),
            &draw(&mut app, 120, 30),
        );
        // The plaintext kind of both rows is `login`, so the search has to carry
        // the key words itself.
        for (word, kept) in [("ssh", "aws"), ("gpg", "netflix"), ("ed25519", "aws")] {
            filter(&mut app, word);
            let found = app
                .home_rows()
                .iter()
                .map(|row| row.id().to_owned())
                .collect::<Vec<_>>();
            assert_eq!(found, vec![kept.to_owned()], "{word}");
        }
    }
}

#[test]
fn the_clipboard_only_reaches_public_metadata() {
    let ssh = ssh_summary();
    let gpg = gpg_summary();
    // Exactly the four values y and Y may hand over, proven without a real copy.
    assert_eq!(public_field(&ssh, false), ssh.public_key.as_str());
    assert_eq!(public_field(&ssh, true), ssh.fingerprint.as_str());
    assert_eq!(public_field(&gpg, false), gpg.comment.as_str());
    assert_eq!(public_field(&gpg, true), gpg.fingerprint.as_str());
    for banner in KEY_MATERIAL {
        for key in [&ssh, &gpg] {
            for fingerprint in [false, true] {
                assert!(!public_field(key, fingerprint).contains(banner));
            }
        }
        // The summary has no field that could hold key text even by accident.
        assert!(!format!("{ssh:?} {gpg:?}").contains(banner));
    }
    // A token row is not a key row, so y stops before the clipboard: the gateway
    // token never becomes copy text on any path that does not select a key first.
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::En,
    );
    app.apply(browsed_keys(docs_library(), vec![ssh.clone(), gpg.clone()]));
    let rows = app.home_rows();
    app.home_selected = rows.iter().position(|row| row.id() == "gitlab").unwrap();
    for fingerprint in [false, true] {
        app.copy_public_field(fingerprint);
        assert_eq!(app.message, app.language.text(Message::ClipboardEmpty));
        assert!(app.failed, "the refusal has to stay on screen");
    }
}

/// The whole chain with nothing mocked out: a real vault on disk, a real generated
/// Ed25519 pair and a real imported ring, the real keystrokes, the real Win32 clipboard,
/// and a read-back from a separate process. Opt-in because it overwrites whatever the
/// developer last copied.
#[test]
#[cfg(windows)]
fn the_real_copy_keys_put_exactly_the_listed_public_field_on_the_clipboard() {
    use crate::keys::manage;
    use crate::vault::Vault;
    use mdbx_core::tiga::TigaMode;

    if std::env::var("MONICA_CLIPBOARD_TEST").as_deref() != Ok("1") {
        eprintln!("set MONICA_CLIPBOARD_TEST=1 to write the real clipboard");
        return;
    }
    let password = crate::test_support::PASSWORD;
    let directory = tempfile::tempdir().unwrap();
    let vault_path = directory.path().join("clipboard-keys.mdbx");
    Vault::create(&vault_path, password, TigaMode::Multi)
        .unwrap()
        .lock()
        .unwrap();
    let store = ConfigStore::new(directory.path().join("gateway.json"));
    store
        .update(|_| Ok((Config::new(vault_path.clone()), ())))
        .unwrap();
    manage::generate_ssh(
        &store,
        password,
        "跳板机 key",
        "验收",
        None,
        "ed25519",
        "joey@interop",
    )
    .unwrap();
    manage::import_gpg_text(
        &store,
        password,
        "归档 ring",
        "验收",
        None,
        include_str!("../../tests/fixtures/gpg/rsa2048-secret.asc"),
    )
    .unwrap();
    // What `keys list --json` reports is the reference for every assertion below.
    let listed = manage::list(&store, password).unwrap();
    assert_eq!(listed.len(), 2, "{listed:?}");
    let (library, keys) = crate::library::read_with_keys(&store, password).unwrap();
    assert_eq!(keys, listed, "the TUI must read the same fields as the CLI");

    let mut app = App::new(ConfigStore::new(store.path.clone()), Language::En);
    app.apply(browsed_keys(library, keys));
    for name in ["跳板机 key", "归档 ring"] {
        let entry = listed.iter().find(|key| key.title == name).unwrap();
        let rows = app.home_rows();
        app.home_selected = rows
            .iter()
            .position(|row| row.id() == entry.entry_id)
            .unwrap_or_else(|| panic!("{name} is not a visible row"));
        let ssh = entry.login_type == crate::keys::payload::LOGIN_TYPE_SSH;
        app.key(key(KeyCode::Char('y')));
        assert_eq!(
            app.message,
            app.language.text(if ssh {
                Message::CopiedPublicKey
            } else {
                Message::CopiedUid
            })
        );
        let public = read_clipboard().expect("PowerShell could not read the clipboard");
        app.key(key(KeyCode::Char('Y')));
        assert_eq!(app.message, app.language.text(Message::CopiedFingerprint));
        let fingerprint = read_clipboard().expect("PowerShell could not read the clipboard");
        let expected: &str = if ssh {
            &entry.public_key
        } else {
            &entry.comment
        };
        assert_eq!(public, expected);
        assert_eq!(fingerprint, entry.fingerprint.as_str());
        for banner in KEY_MATERIAL {
            assert!(!public.contains(banner) && !fingerprint.contains(banner));
        }
        let screen = text(&mut app, 120, 30);
        for banner in KEY_MATERIAL {
            assert!(!screen.contains(banner), "rendered frame leaked {banner}");
        }
        capture_buffer(&format!("clipboard-{name}"), &draw(&mut app, 120, 30));
        eprintln!("clipboard after y: {public}\nclipboard after Y: {fingerprint}");
    }
}

#[cfg(windows)]
fn read_clipboard() -> Option<String> {
    // A separate process, so nothing in this crate's own memory vouches for the write.
    let output = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            "[Console]::OutputEncoding=[Text.Encoding]::UTF8;Get-Clipboard",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_owned(),
    )
}

#[test]
fn search_puts_the_word_start_match_first_even_against_a_tree_order() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::En,
    );
    app.library = Some(ranking_library(&[
        ("openssh", "openssh-server"),
        ("sshkey", "ssh-key"),
    ]));
    // Both hold `ssh`, so an old substring search would have shown both in tree
    // order; ranking has to move the one that starts the word up.
    filter(&mut app, "ssh");
    assert_eq!(ids(&app), vec!["sshkey", "openssh"]);
}

#[test]
fn equally_good_matches_stay_in_tree_order() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::En,
    );
    app.library = Some(ranking_library(&[
        ("tasklist", "task-list"),
        ("tabletop", "table-top"),
    ]));
    // Same anchor, same run, same gap, so the names break the tie instead.
    filter(&mut app, "ta");
    assert_eq!(ids(&app), vec!["tabletop", "tasklist"]);
}

#[test]
fn ranking_a_crowded_tree_leaves_the_cursor_on_the_same_record() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::En,
    );
    app.library = Some(crate::library::Library {
        categories: (0..12)
            .map(|index| crate::library::Category {
                id: format!("cat-{index}"),
                parent: None,
                title: format!("分类 {index} team"),
            })
            .collect(),
        entries: (0..5_000)
            .map(|index| crate::library::Entry {
                id: format!("entry-{index}"),
                category: format!("cat-{}", index % 12),
                title: format!("service {index} 令牌 token"),
                kind: "login".into(),
            })
            .collect(),
    });
    let rows = app.home_rows();
    assert_eq!(rows.len(), 5_012, "every category and entry is one row");
    app.home_selected = rows
        .iter()
        .position(|row| row.id() == "entry-1234")
        .unwrap();
    // Typed one character at a time, which is the path a keystroke actually takes.
    app.key(key(KeyCode::Char('/')));
    let started = Instant::now();
    for code in "token".chars() {
        app.key(key(KeyCode::Char(code)));
    }
    let broad = started.elapsed();
    for code in " 12".chars() {
        app.key(key(KeyCode::Char(code)));
    }
    let narrowed = started.elapsed();
    app.key(key(KeyCode::Enter));
    let found: Vec<_> = app
        .home_rows()
        .iter()
        .map(|row| row.id().to_owned())
        .collect();
    // Every row matched the first term, so this list is entirely the second term's
    // doing: `12` right after a separator and run together has to lead.
    assert_eq!(
        found.first().map(String::as_str),
        Some("entry-12"),
        "{found:?}"
    );
    assert!(found.contains(&"entry-1234".to_owned()));
    // Re-ranking must not hand the cursor to another record.
    assert_eq!(app.selected_home_row().unwrap().id(), "entry-1234");
    let started = Instant::now();
    let screen = text(&mut app, 120, 30);
    let drawn = started.elapsed();
    assert!(screen.contains("service 12"), "{screen}");
    eprintln!(
        "5,012 rows: 5 keystrokes over every row {broad:?}, 8 keystrokes {narrowed:?}, \
         one 120x30 frame {drawn:?}"
    );
}

fn ranking_library(entries: &[(&str, &str)]) -> crate::library::Library {
    crate::library::Library {
        categories: vec![crate::library::Category {
            id: "dev".into(),
            parent: None,
            title: "开发".into(),
        }],
        entries: entries
            .iter()
            .map(|(id, title)| crate::library::Entry {
                id: (*id).into(),
                category: "dev".into(),
                title: (*title).into(),
                kind: "login".into(),
            })
            .collect(),
    }
}

fn ids(app: &App) -> Vec<String> {
    app.home_rows()
        .iter()
        .map(|row| row.id().to_owned())
        .collect()
}

#[test]
fn key_commands_stop_at_the_database_they_need() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::En,
    );
    // Without a gateway config there is no database to write into at all.
    for command in ["ssh", "sshimport", "gpg"] {
        app.invoke(command);
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.pending.is_none());
    }
    app.config = Some(Config::new(directory.path().join("synthetic.mdbx")));
    for command in ["ssh", "sshimport", "gpg"] {
        app.invoke(command);
        assert!(
            matches!(&app.mode, Mode::Form(form) if matches!(form.kind, Kind::Library)),
            "{command} skipped the unlock"
        );
        assert_eq!(
            app.message,
            app.language.text(Message::KeyNeedsOpenedDatabase)
        );
        app.mode = Mode::Normal;
    }
    app.library = Some(docs_library());
    for (command, kind) in [("ssh", true), ("sshimport", false)] {
        app.invoke(command);
        let Mode::Form(form) = &app.mode else {
            panic!("{command} opened no form")
        };
        assert_eq!(matches!(form.kind, Kind::AddSsh { generate: true }), kind);
        app.mode = Mode::Normal;
    }
    app.invoke("gpg");
    assert!(matches!(&app.mode, Mode::Form(form) if matches!(form.kind, Kind::AddGpg)));
}

#[test]
fn pasted_armor_stays_masked_and_reports_only_its_line_count() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::ZhCn,
    );
    app.config = Some(Config::new(directory.path().join("synthetic.mdbx")));
    app.library = Some(docs_library());
    app.invoke("sshimport");
    {
        let Mode::Form(form) = &mut app.mode else {
            panic!("import opened no form")
        };
        form.fields[0].input.insert("跳板机");
        form.selected = 1;
        form.paste("-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAAbW9jaw==\n-----END OPENSSH PRIVATE KEY-----\n");
    }
    let screen = text(&mut app, 100, 30);
    for banner in KEY_MATERIAL {
        assert!(!screen.contains(banner), "key text reached the screen");
    }
    assert!(!screen.contains("b3BlbnNzaC1r"), "{screen}");
    // The masked field still proves the paste was not truncated.
    assert!(screen.contains("已粘贴 3 行"), "{screen}");
    assert!(screen.contains('*'), "{screen}");
    capture_buffer("form-ssh-paste-zh", &draw(&mut app, 120, 30));
    app.key(key(KeyCode::Enter));
    let Mode::Form(form) = &app.mode else {
        panic!("Enter submitted the key form")
    };
    assert!(app.pending.is_none());
    assert_eq!(form.fields[1].input.lines(), 4);
    assert_modal_survives_resize(&mut app);
}

#[test]
fn home_states_render_the_tree_search_and_detail() {
    for language in [Language::En, Language::ZhCn] {
        let directory = tempfile::tempdir().unwrap();
        let mut app = App::new(
            ConfigStore::new(directory.path().join("gateway.json")),
            language,
        );
        app.nerd_font = true;
        app.databases = crowded_databases(directory.path());
        let tag = if language == Language::En { "en" } else { "zh" };
        // A locked database still offers selectable actions, not a wall of hints,
        // and the first action depends on whether a gateway config is registered.
        let locked = text(&mut app, 100, 30);
        assert!(locked.contains(language.text(Message::NewDatabaseRow)));
        assert!(locked.contains(language.text(Message::OpenLocalRow)));
        capture_buffer(&format!("home-{tag}-locked"), &draw(&mut app, 100, 30));
        app.config = Some(Config::new(directory.path().join("synthetic.mdbx")));
        let registered = text(&mut app, 100, 30);
        assert!(registered.contains(language.text(Message::OpenDatabaseRow)));
        capture_buffer(&format!("home-{tag}-registered"), &draw(&mut app, 100, 30));
        app.apply(browsed(crowded_library()));
        // `apply` reloads the registry from disk, which a temp store does not have.
        app.databases = crowded_databases(directory.path());
        capture_buffer(&format!("home-{tag}-tree"), &draw(&mut app, 100, 30));
        let tree = text(&mut app, 100, 30);
        assert!(tree.contains("工作项目"));
        assert!(tree.contains("GitLab"));
        if language == Language::En {
            assert!(
                tree.contains("0 tokens") && !tree.contains("1 tokens"),
                "{tree}"
            );
        } else {
            assert!(tree.contains("1 个 Token"), "{tree}");
        }
        app.key(key(KeyCode::Char('d')));
        assert_eq!(app.focus, Focus::Navigation);
        capture_buffer(&format!("home-{tag}-rail"), &draw(&mut app, 100, 30));
        app.key(key(KeyCode::Tab));
        app.key(key(KeyCode::Char('/')));
        for ch in "令牌".chars() {
            app.key(key(KeyCode::Char(ch)));
        }
        let ids: Vec<String> = app
            .home_rows()
            .iter()
            .map(|row| row.id().to_owned())
            .collect();
        assert_eq!(ids, ["token"]);
        let search = text(&mut app, 100, 30);
        assert!(search.contains("GitLab") && !search.contains("no-category-entry"));
        capture_buffer(&format!("home-{tag}-search"), &draw(&mut app, 100, 30));
        app.key(key(KeyCode::Esc));
        assert!(app.home_filter.is_empty());
        for _ in 0..3 {
            app.key(key(KeyCode::Enter));
        }
        assert_eq!(app.focus, Focus::Preview);
        let detail = render(&mut app, 100, 30);
        assert!(detail.contains("••••••••"));
        assert!(detail.contains("monica-pass"));
        capture_buffer(&format!("home-{tag}-detail"), &draw(&mut app, 100, 30));
    }
}

#[test]
fn home_docs_screen_is_captured_for_the_readme() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::ZhCn,
    );
    app.nerd_font = true;
    app.databases = docs_databases(directory.path());
    app.apply(browsed(docs_library()));
    // `apply` reloads the registry from disk, which a temp store does not have.
    app.databases = docs_databases(directory.path());
    for _ in 0..8 {
        if app
            .selected_home_row()
            .is_some_and(|row| row.id() == "gitlab")
        {
            break;
        }
        app.key(key(KeyCode::Char('j')));
    }
    assert_eq!(
        app.selected_home_row().map(|row| row.id().to_owned()),
        Some("gitlab".into())
    );
    assert_rails(&mut app, 100, 30);
    let screen = text(&mut app, 100, 30);
    assert!(
        screen.contains("GitLab") && screen.contains("monica-pass"),
        "{screen}"
    );
    capture_buffer("home-docs", &draw(&mut app, 100, 30));
}

fn docs_databases(directory: &std::path::Path) -> Vec<crate::databases::Database> {
    [
        ("personal", "个人库", true),
        ("work", "工作", false),
        ("lab", "实验室", false),
    ]
    .into_iter()
    .map(|(id, name, current)| crate::databases::Database {
        id: id.into(),
        name: name.into(),
        path: directory.join(format!("{id}.mdbx")),
        current,
    })
    .collect()
}

fn browsed(library: crate::library::Library) -> Outcome {
    browsed_keys(library, Vec::new())
}

fn browsed_keys(
    library: crate::library::Library,
    keys: Vec<crate::vault::KeyEntrySummary>,
) -> Outcome {
    Outcome::Library {
        library,
        keys,
        saved: None,
    }
}

fn docs_library() -> crate::library::Library {
    crate::library::Library {
        categories: vec![
            crate::library::Category {
                id: "dev".into(),
                parent: None,
                title: "开发".into(),
            },
            crate::library::Category {
                id: "pass".into(),
                parent: Some("dev".into()),
                title: "monica-pass".into(),
            },
            crate::library::Category {
                id: "life".into(),
                parent: None,
                title: "日常".into(),
            },
        ],
        entries: vec![
            crate::library::Entry {
                id: "gitlab".into(),
                category: "pass".into(),
                title: "GitLab 内部实例".into(),
                kind: "api-token".into(),
            },
            crate::library::Entry {
                id: "aws".into(),
                category: "dev".into(),
                title: "AWS 控制台".into(),
                kind: "login".into(),
            },
            crate::library::Entry {
                id: "netflix".into(),
                category: "life".into(),
                title: "Netflix".into(),
                kind: "login".into(),
            },
        ],
    }
}

fn crowded_databases(directory: &std::path::Path) -> Vec<crate::databases::Database> {
    [
        ("personal", "个人库 · 主用", true),
        ("work", "工作项目 work 🔐", false),
    ]
    .into_iter()
    .map(|(id, name, current)| crate::databases::Database {
        id: id.into(),
        name: name.into(),
        path: directory.join(format!("{id}.mdbx")),
        current,
    })
    .collect()
}

fn crowded_library() -> crate::library::Library {
    crate::library::Library {
        categories: vec![
            crate::library::Category {
                id: "root".into(),
                parent: None,
                title: "工作项目".into(),
            },
            crate::library::Category {
                id: "child".into(),
                parent: Some("root".into()),
                title: "monica-pass 🔐".into(),
            },
        ],
        entries: vec![
            crate::library::Entry {
                id: "token".into(),
                category: "child".into(),
                title: "GitLab 内部实例访问令牌".into(),
                kind: "api-token".into(),
            },
            crate::library::Entry {
                id: "loose".into(),
                category: "missing".into(),
                title: "no-category-entry".into(),
                kind: "login".into(),
            },
        ],
    }
}

fn ssh_summary() -> crate::vault::KeyEntrySummary {
    crate::vault::KeyEntrySummary {
        entry_id: "aws".into(),
        logical_id: "password:22222222-2222-4222-8222-222222222222".into(),
        collection_id: "dev".into(),
        title: "AWS 控制台".into(),
        login_type: crate::keys::payload::LOGIN_TYPE_SSH.into(),
        algorithm: "ED25519".into(),
        key_size: Some(256),
        fingerprint: "SHA256:sshmockfingerprint".into(),
        comment: "joey@work".into(),
        public_key: "ssh-ed25519 AAAAMOCKPUBLIC joey@work".into(),
        notes: String::new(),
        has_secret: true,
        chunk_count: 0,
    }
}

fn gpg_summary() -> crate::vault::KeyEntrySummary {
    crate::vault::KeyEntrySummary {
        entry_id: "netflix".into(),
        logical_id: "password:33333333-3333-4333-8333-333333333333".into(),
        collection_id: "life".into(),
        title: "Netflix".into(),
        login_type: crate::keys::payload::LOGIN_TYPE_GPG.into(),
        algorithm: "RSA".into(),
        key_size: Some(3072),
        fingerprint: "0123456789abcdef0123456789abcdef01234567".into(),
        comment: "Joyin Joester <joyin@example.com>".into(),
        public_key: String::new(),
        notes: String::new(),
        has_secret: false,
        chunk_count: 2,
    }
}

fn assert_rails(app: &mut App, width: u16, height: u16) -> Vec<u16> {
    let screen = view::chrome(Rect::new(0, 0, width, height));
    let rails = [screen.columns[1].x, screen.columns[1].right() - 1];
    let buffer = draw(app, width, height);
    for row in 0..screen.body.height {
        let y = screen.body.y + row;
        let found: Vec<u16> = (0..width)
            .filter(|x| buffer[(*x, y)].symbol() == "│")
            .collect();
        assert_eq!(
            found,
            rails.to_vec(),
            "rails drifted at {width}x{height} row {row}"
        );
    }
    assert_eq!(
        buffer,
        draw(app, width, height),
        "a redraw differs from a clean frame at {width}x{height}"
    );
    rails.to_vec()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}
fn draw(app: &mut App, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| view::render(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}
fn render(app: &mut App, width: u16, height: u16) -> String {
    draw(app, width, height)
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

/// Screen text with the pad half of every wide glyph dropped, so translated
/// CJK labels stay contiguous instead of reading "工 作 项 目". Rows are kept
/// apart by newlines, so a match cannot span two of them.
fn text(app: &mut App, width: u16, height: u16) -> String {
    let buffer = draw(app, width, height);
    let width = width as usize;
    let mut skip = 0;
    let mut screen = String::new();
    for (index, cell) in buffer.content.iter().enumerate() {
        if index % width == 0 {
            screen.push('\n');
        }
        if skip > 0 {
            skip -= 1;
            continue;
        }
        let symbol = cell.symbol();
        skip = ratatui::text::Line::raw(symbol).width().saturating_sub(1);
        screen.push_str(symbol);
    }
    screen
}

fn manager_fixture(directory: &std::path::Path) -> App {
    use crate::config::{Connection, connection_fingerprint};
    use crate::model::{Operation, Provider};

    let store = ConfigStore::new(directory.join("gateway.json"));
    let mut config = Config::new(directory.join("synthetic.mdbx"));
    for (name, provider, note) in [
        (
            "docs-github",
            Provider::Github,
            "维护文档与 Issue 跟踪，查看发布进度。",
        ),
        (
            "personal-github",
            Provider::Github,
            "个人项目的问题与功能建议。",
        ),
        (
            "work-gitlab",
            Provider::Gitlab,
            "内部项目的问题处理与需求确认。",
        ),
    ] {
        config.connections.insert(
            name.to_owned(),
            Connection {
                provider,
                credential_id: uuid::Uuid::new_v4().to_string(),
                api_base: provider.default_api_base().to_owned(),
                note: note.to_owned(),
            },
        );
    }
    let now = chrono::Utc::now().timestamp();
    for (index, (name, connection, write)) in [
        ("docs-read", "docs-github", false),
        ("work-read", "work-gitlab", false),
        ("work-write", "work-gitlab", true),
    ]
    .into_iter()
    .enumerate()
    {
        let mut operations = [Operation::ListIssues, Operation::GetIssue]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        if write {
            operations.insert(Operation::CreateIssue);
        }
        config.grants.push(Grant {
            name: name.to_owned(),
            connection: connection.to_owned(),
            capability_hash: format!("{index:064x}"),
            connection_fingerprint: connection_fingerprint(&config.connections[connection]),
            repositories: ["team/monica".to_owned()].into_iter().collect(),
            operations,
            issued_at: now - 60,
            expires_at: if index == 0 { now - 1 } else { now + 3600 },
            requests_per_minute: 60,
            max_calls: 0,
            approval: crate::model::ApprovalPolicy::Off,
            client_file: Some(directory.join(format!("{name}.client.json"))),
        });
    }
    store.update(|_| Ok((config, ()))).unwrap();
    let mut app = App::new(store, Language::ZhCn);
    app.key(key(KeyCode::F(3)));
    app.set_page(Page::Connections);
    app.nerd_font = true;
    app
}

fn filter(app: &mut App, query: &str) {
    app.key(key(KeyCode::Char('/')));
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    app.paste(query);
    app.key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn tui_language_switch_preserves_selection_form_values_and_secret_buffers() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = manager_fixture(directory.path());
    filter(&mut app, "work-gitlab");
    let config_before = std::fs::read(&app.store.path).unwrap();
    let selected = app.selected_connection();
    app.key(key(KeyCode::F(2)));
    assert_eq!(app.language, Language::En);
    assert_eq!(app.selected_connection(), selected);
    assert_eq!(
        app.filters[Page::Connections.index()].as_str(),
        "work-gitlab"
    );
    assert!(render(&mut app, 120, 30).contains("Purpose"));
    assert_eq!(
        Preferences::load(&app.store).unwrap().language,
        LanguageChoice::En
    );
    assert_eq!(std::fs::read(&app.store.path).unwrap(), config_before);
    app.show_form(Kind::Note);
    let Mode::Form(form) = &mut app.mode else {
        panic!("expected form")
    };
    form.fields[1].input = Input::new("Public note {language} 👩‍💻", 4096);
    form.fields[2].input = Input::new("synthetic-secret-for-i18n-test", 4096);
    form.fields[2].input.cursor = 5;
    let secret_buffer = form.fields[2].input.value.as_ptr();
    form.selected = 2;
    form.insert = false;
    app.key(key(KeyCode::F(2)));
    assert_eq!(app.language, Language::ZhCn);
    let Mode::Form(form) = &app.mode else {
        panic!("form was lost")
    };
    assert_eq!(form.title, "编辑 AI 可见备注");
    assert_eq!(
        form.fields[1].input.value.as_str(),
        "Public note {language} 👩‍💻"
    );
    assert_eq!(
        form.fields[2].input.value.as_str(),
        "synthetic-secret-for-i18n-test"
    );
    assert_eq!(form.fields[2].input.value.as_ptr(), secret_buffer);
    assert_eq!(form.fields[2].input.cursor, 5);
    assert_eq!(form.selected, 2);
    assert!(!form.insert);
    let screen = render(&mut app, 80, 24);
    assert!(!screen.contains("synthetic-secret-for-i18n-test"));
    assert!(screen.replace(' ', "").contains("隐藏"));
    assert_eq!(std::fs::read(&app.store.path).unwrap(), config_before);
    assert!(app.pending.is_none());
    assert_modal_survives_resize(&mut app);
}

#[test]
fn tui_language_filtering_and_completed_actions_use_the_current_language() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = manager_fixture(directory.path());
    app.invoke("lang en");
    app.set_page(Page::Grants);
    filter(&mut app, "read/write");
    assert_eq!(app.rows(), 1);
    assert_eq!(app.selected_grant().unwrap().name, "work-write");
    app.apply(Outcome::Message(Message::NoteUpdatedHint));
    assert!(app.message.contains("AI-visible note updated"));
    app.invoke("lang zh-CN");
    // A translated status filter can cease matching, but must never act on
    // a different, unfiltered grant after the language changes.
    assert_eq!(app.rows(), 0);
    assert!(app.selected_grant().is_none());
    app.invoke("revoke");
    assert!(matches!(app.mode, Mode::Normal));
    assert!(app.pending.is_none());
    app.apply(Outcome::Message(Message::NoteUpdatedHint));
    assert!(app.message.contains("AI 可见备注已更新"));
    app.invoke("lang unsupported");
    assert_eq!(app.language, Language::ZhCn);
    assert_eq!(
        Preferences::load(&app.store).unwrap().language,
        LanguageChoice::ZhCn
    );
}

#[test]
fn tui_both_languages_cover_all_pages_forms_and_responsive_borders() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = manager_fixture(directory.path());
    for language in [Language::En, Language::ZhCn] {
        app.change_language(language.choice());
        for page in Page::ALL {
            app.set_page(page);
            for (width, height) in [(70, 20), (80, 24), (120, 30)] {
                let buffer = draw(&mut app, width, height);
                let rails: Vec<_> = (0..width)
                    .filter(|x| buffer[(*x, 1)].symbol() == "│")
                    .collect();
                assert_eq!(rails.len(), 2);
                for y in 1..height - 1 {
                    assert!(rails.iter().all(|x| buffer[(*x, y)].symbol() == "│"));
                }
                if page == Page::Connections && width == 120 {
                    capture_buffer(
                        &format!("i18n-{}-manager", language.choice().code()),
                        &buffer,
                    );
                }
            }
        }
        for kind in [
            Kind::Add { new_vault: true },
            Kind::Add { new_vault: false },
            Kind::Note,
            Kind::Init,
            Kind::OpenLocal,
            Kind::Connect,
            Kind::Grant,
            Kind::Login,
            Kind::OpenRemote("中文/vault.mdbx".to_owned()),
            Kind::Publish,
            Kind::Unlock,
            Kind::Sync,
            Kind::Revoke,
            Kind::Delete {
                target: actions::DeleteTarget::Entry("aws".to_owned()),
                expect: "AWS 控制台".to_owned(),
            },
            Kind::AddSsh { generate: true },
            Kind::AddSsh { generate: false },
            Kind::AddGpg,
            Kind::EditKey {
                entry_id: "aws".to_owned(),
                login_type: crate::keys::payload::LOGIN_TYPE_SSH.to_owned(),
                title: "AWS 控制台".to_owned(),
                comment: "joey@work".to_owned(),
                notes: String::new(),
            },
        ] {
            let capture = match &kind {
                Kind::AddSsh { generate: true } => Some("form-ssh-generate"),
                Kind::AddSsh { generate: false } => Some("form-ssh-import"),
                Kind::AddGpg => Some("form-gpg-import"),
                Kind::EditKey { .. } => Some("form-key-edit"),
                _ => None,
            };
            app.show_form(kind);
            let Mode::Form(form) = &app.mode else {
                unreachable!()
            };
            if language == Language::En {
                for text in std::iter::once(form.title).chain(
                    form.fields
                        .iter()
                        .flat_map(|field| [field.label, field.hint]),
                ) {
                    assert!(
                        !text
                            .chars()
                            .any(|ch| ('\u{3400}'..='\u{9fff}').contains(&ch)),
                        "{text}"
                    );
                }
            }
            assert_modal_survives_resize(&mut app);
            if let Some(name) = capture {
                capture_buffer(
                    &format!("{name}-{}", language.choice().code()),
                    &draw(&mut app, 120, 30),
                );
            }
        }
        app.show_form(Kind::Login);
        capture_buffer(
            &format!("i18n-{}-webdav", language.choice().code()),
            &draw(&mut app, 120, 30),
        );
        app.mode = Mode::Normal;
        app.show_help();
        assert_modal_survives_resize(&mut app);
        app.mode = Mode::Normal;
    }
}

#[tokio::test]
async fn tui_filtered_records_bind_edit_grant_and_revoke_to_visible_names() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = manager_fixture(directory.path());
    filter(&mut app, "GITLAB 内部");
    assert_eq!(app.rows(), 1);
    assert_eq!(app.selected_connection().as_deref(), Some("work-gitlab"));
    app.key(key(KeyCode::Char('e')));
    assert!(
        matches!(&app.mode, Mode::Form(form) if form.fields[0].input.value.as_str() == "work-gitlab"
        && form.fields[1].input.value.contains("内部"))
    );
    app.key(key(KeyCode::Esc));
    app.key(key(KeyCode::Esc));
    app.key(key(KeyCode::Char('a')));
    assert!(
        matches!(&app.mode, Mode::Form(form) if form.fields[1].input.value.as_str() == "work-gitlab")
    );
    app.key(key(KeyCode::Esc));
    app.key(key(KeyCode::Esc));
    filter(&mut app, "no-such-connection");
    assert!(app.selected_connection().is_none());
    for command in ['e', 'a'] {
        app.key(key(KeyCode::Char(command)));
        assert!(matches!(app.mode, Mode::Normal));
    }
    app.key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Normal));

    app.key(key(KeyCode::Char('3')));
    filter(&mut app, "work-write team/monica");
    assert_eq!(app.selected_index(Page::Grants), Some(2));
    let (name, path) = app.grant_client().unwrap();
    assert_eq!(name, "work-write");
    assert_eq!(path, directory.path().join("work-write.client.json"));
    app.key(key(KeyCode::Char('x')));
    assert!(
        matches!(&app.mode, Mode::Form(form) if form.fields[0].input.value.as_str() == "work-write")
    );
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app).await;
    let config = app.store.load().unwrap();
    assert_eq!(
        config
            .grants
            .iter()
            .map(|grant| grant.name.as_str())
            .collect::<Vec<_>>(),
        ["docs-read", "work-read"]
    );
    // The exact-name row is gone and the cursor moves to the next best match, which
    // a subsequence search keeps listed.
    assert_eq!(app.selected_index(Page::Grants), Some(1));
    assert_eq!(
        app.selected_grant().map(|grant| grant.name.as_str()),
        Some("work-read")
    );
    filter(&mut app, "qqzzxx");
    assert!(app.selected_grant().is_none());
    for command in ['x', 'm', 'p'] {
        app.key(key(KeyCode::Char(command)));
        assert!(matches!(app.mode, Mode::Normal));
        assert!(app.pending.is_none());
    }
}

#[test]
fn tui_filter_typing_paste_and_pane_navigation_do_not_execute_actions() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = manager_fixture(directory.path());
    app.key(key(KeyCode::Char('/')));
    for ch in "qxc".chars() {
        app.key(key(KeyCode::Char(ch)));
    }
    app.paste("\u{001b}\r\n:login");
    app.paste(&"密".repeat(200));
    assert!(matches!(app.mode, Mode::Filter(_)));
    assert!(app.filters[Page::Connections.index()].len() <= 256);
    assert!(
        !app.filters[Page::Connections.index()]
            .chars()
            .any(char::is_control)
    );
    assert!(!app.quitting && app.pending.is_none());
    app.key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Normal));
    assert_eq!(app.rows(), 0);
    app.key(key(KeyCode::Esc));
    app.key(key(KeyCode::Char('G')));
    let before = app.selection_key(Page::Connections);
    app.key(key(KeyCode::Char('/')));
    app.paste("docs");
    assert_eq!(app.rows(), 1);
    app.key(key(KeyCode::Esc));
    assert_eq!(app.selection_key(Page::Connections), before);
    assert!(app.filters[Page::Connections.index()].is_empty());

    app.key(key(KeyCode::Char('h')));
    assert!(app.focus == Focus::Navigation);
    app.key(key(KeyCode::Char('j')));
    assert!(app.page == Page::Grants);
    app.key(key(KeyCode::Enter));
    assert!(app.focus == Focus::List && matches!(app.mode, Mode::Normal));
    app.key(key(KeyCode::BackTab));
    app.key(key(KeyCode::Char('G')));
    assert!(app.page == Page::Help);
    app.key(key(KeyCode::Char('g')));
    assert!(app.page == Page::Dashboard);
    app.key(key(KeyCode::Char('5')));
    draw(&mut app, 70, 20);
    app.move_selection(true, 15);
    draw(&mut app, 70, 20);
    let offset = app.list_offsets[Page::Help.index()];
    assert!(offset > 0);
    app.move_selection(false, 1);
    draw(&mut app, 70, 20);
    assert_eq!(app.list_offsets[Page::Help.index()], offset);
    filter(&mut app, "revoke");
    assert_eq!(app.rows(), 1);
    assert!(matches!(app.mode, Mode::Normal));
    app.key(key(KeyCode::Enter));
    // Command discovery navigates to the record picker before a destructive action.
    assert!(app.page == Page::Grants && matches!(app.mode, Mode::Normal));
    app.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));
    app.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert!(matches!(app.mode, Mode::Normal));
}

#[test]
fn tui_refresh_preserves_selected_identity_when_config_order_changes() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = manager_fixture(directory.path());
    app.key(key(KeyCode::Char('G')));
    app.store
        .update(|config| {
            let mut config = config.unwrap();
            let copy = config.connections["docs-github"].clone();
            config.connections.insert("a-new-first".to_owned(), copy);
            config.grants.reverse();
            Ok((config, ()))
        })
        .unwrap();
    app.reload();
    assert_eq!(app.selected_connection().as_deref(), Some("work-gitlab"));
    app.set_page(Page::Grants);
    assert_eq!(app.selected_grant().unwrap().name, "docs-read");
    filter(&mut app, "work-read");
    app.store
        .update(|config| {
            let mut config = config.unwrap();
            config.grants.reverse();
            Ok((config, ()))
        })
        .unwrap();
    app.reload();
    assert_eq!(app.selected_grant().unwrap().name, "work-read");
}

#[test]
fn a_spent_call_budget_is_never_drawn_as_an_active_grant() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = manager_fixture(directory.path());
    app.store
        .update(|config| {
            let mut config = config.unwrap();
            config
                .grants
                .iter_mut()
                .find(|grant| grant.name == "work-read")
                .unwrap()
                .max_calls = 2;
            Ok((config, ()))
        })
        .unwrap();
    let hash = app
        .config
        .as_ref()
        .unwrap()
        .grants
        .iter()
        .find(|grant| grant.name == "work-read")
        .unwrap()
        .capability_hash
        .clone();
    app.reload();
    app.set_page(Page::Grants);
    app.selected[Page::Grants.index()] = 0;
    let name_color = |app: &mut App, needle: &str| {
        let buffer = draw(app, 100, 30);
        let row = buffer
            .content
            .chunks(100)
            .position(|line| {
                line.iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>()
                    .contains(needle)
            })
            .unwrap_or_else(|| panic!("{needle} was not drawn"));
        let cells = &buffer.content[row * 100..(row + 1) * 100];
        let line: String = cells.iter().map(|cell| cell.symbol()).collect();
        cells[line[..line.find(needle).unwrap()].chars().count()].fg
    };
    assert_eq!(
        name_color(&mut app, "work-read"),
        view::ACCENT,
        "an unspent budget is still a live grant"
    );

    std::fs::write(app.store.usage_path(), format!(r#"{{"{hash}":2}}"#)).unwrap();
    app.reload();
    let screen = text(&mut app, 100, 30);
    assert!(
        screen.contains(app.language.text(Message::GrantExhausted)),
        "{screen}"
    );
    assert_eq!(
        name_color(&mut app, "work-read"),
        view::DIM,
        "a refused grant must not look live"
    );
    assert_eq!(name_color(&mut app, "work-write"), view::ACCENT);

    app.selected[Page::Grants.index()] = 1;
    let detail = text(&mut app, 100, 30);
    assert!(detail.contains("2/2"), "{detail}");
}

#[test]
fn the_home_tree_routes_to_the_grant_list_without_unlocking() {
    for language in [Language::En, Language::ZhCn] {
        let directory = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(directory.path().join("gateway.json"));
        store
            .update(|_| Ok((Config::new(directory.path().join("synthetic.mdbx")), ())))
            .unwrap();
        let mut app = App::new(store, language);
        assert!(app.library.is_none(), "the vault stays locked here");
        let rows: Vec<String> = app
            .home_rows()
            .iter()
            .map(|row| row.id().to_owned())
            .collect();
        assert_eq!(
            rows.last().map(String::as_str),
            Some("action:grants"),
            "{rows:?}"
        );
        assert!(
            text(&mut app, 100, 30).contains(language.text(Message::PageGrants)),
            "{language:?} locked"
        );
        app.home_selected = rows.len() - 1;
        app.key(key(KeyCode::Enter));
        assert!(app.page == Page::Grants);
        assert!(!app.home);

        app.key(key(KeyCode::F(3)));
        app.apply(browsed(crowded_library()));
        let tree: Vec<String> = app
            .home_rows()
            .iter()
            .map(|row| row.id().to_owned())
            .collect();
        assert_eq!(
            tree.first().map(String::as_str),
            Some("action:grants"),
            "{tree:?}"
        );
        app.update_home_filter(if language == Language::En {
            "grant"
        } else {
            "授权"
        });
        assert!(
            app.home_rows()
                .iter()
                .any(|row| row.id() == "action:grants"),
            "{language:?} search"
        );
    }
}

#[test]
fn tui_filtered_webdav_opens_exact_path_and_resets_search_on_folder_change() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = manager_fixture(directory.path());
    app.set_page(Page::WebDav);
    app.webdav = Some(
        WebDavClient::new(
            WebDavProfile::new("https://dav.example.test/", "synthetic").unwrap(),
            Zeroizing::new("synthetic-unused-password".to_owned()),
        )
        .unwrap(),
    );
    let first = RemoteEntry {
        path: "first.mdbx".to_owned(),
        is_directory: false,
        size: Some(4096),
        etag: Some("\"v1\"".to_owned()),
    };
    let chosen = RemoteEntry {
        path: "chosen.mdbx".to_owned(),
        ..first.clone()
    };
    app.entries = vec![first.clone(), chosen.clone()];
    filter(&mut app, "chosen");
    app.key(key(KeyCode::Enter));
    assert!(
        matches!(&app.mode, Mode::Form(Form {kind: Kind::OpenRemote(path), ..}) if path == "chosen.mdbx")
    );
    app.key(key(KeyCode::Esc));
    app.key(key(KeyCode::Esc));
    filter(&mut app, "missing");
    app.key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Normal) && app.pending.is_none());
    filter(&mut app, "chosen");
    app.apply(Outcome::Browse {
        path: String::new(),
        entries: vec![chosen.clone(), first.clone()],
    });
    assert_eq!(
        app.selection_key(Page::WebDav).as_deref(),
        Some("chosen.mdbx")
    );
    app.apply(Outcome::Browse {
        path: "another-folder".to_owned(),
        entries: vec![first],
    });
    assert!(app.filters[Page::WebDav.index()].is_empty());
    assert_eq!(
        app.selection_key(Page::WebDav).as_deref(),
        Some("first.mdbx")
    );
}

fn capture_buffer(name: &str, buffer: &ratatui::buffer::Buffer) {
    // Opt-in acceptance artifacts contain only this test's synthetic UI cells.
    let Some(directory) = std::env::var_os("MONICA_TUI_CAPTURE_DIR") else {
        return;
    };
    let directory = PathBuf::from(directory);
    assert!(directory.is_absolute());
    std::fs::create_dir_all(&directory).unwrap();
    let cells: Vec<_> = buffer.content.iter().map(|cell| serde_json::json!({
        "text": cell.symbol(), "fg": format!("{:?}", cell.fg), "bg": format!("{:?}", cell.bg),
        "bold": cell.modifier.contains(ratatui::style::Modifier::BOLD),
        "underline": cell.modifier.contains(ratatui::style::Modifier::UNDERLINED),
        "width": ratatui::text::Line::raw(cell.symbol()).width(),
    })).collect();
    std::fs::write(
        directory.join(format!("{name}.json")),
        serde_json::to_vec(&serde_json::json!({
            "width": buffer.area.width, "height": buffer.area.height, "cells": cells,
            "source": "Ratatui TestBackend, synthetic public metadata",
        }))
        .unwrap(),
    )
    .unwrap();
}

fn assert_modal_survives_resize(app: &mut App) {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    for (width, height) in [
        (80, 24),
        (70, 20),
        (120, 30),
        (71, 21),
        (81, 25),
        (69, 19),
        (80, 24),
    ] {
        terminal.backend_mut().resize(width, height);
        terminal.draw(|frame| view::render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        // A redraw after shrinking/growing must match a clean first frame.
        assert_eq!(buffer, &draw(app, width, height));
        if width < 70 || height < 20 {
            continue;
        }
        let [top_left, top_right, bottom_left, bottom_right] = ["╭", "╮", "╰", "╯"].map(|symbol| {
            let positions: Vec<_> = buffer
                .content
                .iter()
                .enumerate()
                .filter(|(_, cell)| cell.symbol() == symbol)
                .map(|(index, _)| {
                    (
                        (index % width as usize) as u16,
                        (index / width as usize) as u16,
                    )
                })
                .collect();
            if positions.len() != 1 {
                capture_buffer(&format!("border-failure-{width}x{height}"), buffer);
            }
            let title = match &app.mode {
                Mode::Form(form) => form.title,
                Mode::Popup(popup) => &popup.title,
                _ => "not a modal",
            };
            assert_eq!(positions.len(), 1, "{symbol} at {width}x{height}: {title}");
            positions[0]
        });
        assert_eq!(top_left.0, bottom_left.0);
        assert_eq!(top_right.0, bottom_right.0);
        assert_eq!(top_left.1, top_right.1);
        assert_eq!(bottom_left.1, bottom_right.1);
        for y in top_left.1 + 1..bottom_left.1 {
            for x in [top_left.0, top_right.0] {
                assert_eq!(buffer[(x, y)].symbol(), "│", "side at ({x}, {y})");
                assert_eq!(buffer[(x, y)].fg, view::ACCENT);
            }
        }
    }
}

#[test]
fn tui_modal_borders_stay_aligned_with_unicode_and_terminal_resizes() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = manager_fixture(directory.path());
    for kind in [
        Kind::Add { new_vault: true },
        Kind::Add { new_vault: false },
        Kind::Note,
        Kind::Init,
        Kind::OpenLocal,
        Kind::Connect,
        Kind::Grant,
        Kind::Login,
        Kind::OpenRemote("synthetic.mdbx".to_owned()),
        Kind::Publish,
        Kind::Unlock,
        Kind::Sync,
        Kind::Revoke,
    ] {
        let mut form = Form::new(kind, &app);
        for field in &mut form.fields {
            field.input = Input::new(
                if field.secret {
                    "SYNTHETIC-private-value"
                } else {
                    "中文👩‍💻e\u{301}很长的输入与路径/"
                },
                4096,
            );
            if !field.secret {
                field.input.insert(&"中文👩‍💻e\u{301}long/path/".repeat(40));
            }
        }
        let last = form.fields.len() - 1;
        app.mode = Mode::Form(form);
        for selected in [0, last] {
            let Mode::Form(form) = &mut app.mode else {
                unreachable!()
            };
            form.selected = selected;
            assert_modal_survives_resize(&mut app);
        }
    }
    app.mode = Mode::Normal;
    app.key(key(KeyCode::Char('?')));
    assert_modal_survives_resize(&mut app);
    for (width, height) in [(70, 20), (120, 30)] {
        capture_buffer(
            &format!("overlay-help-{width}x{height}"),
            &draw(&mut app, width, height),
        );
    }
    app.mode = Mode::Popup(Popup {
        title: "很长的结果标题👩‍💻".repeat(20),
        lines: vec!["中文👩‍💻e\u{301}结果和长路径/".repeat(120)],
        scroll: 0,
        max_scroll: 0,
        client: None,
    });
    assert_modal_survives_resize(&mut app);
    capture_buffer("overlay-unicode-120x30", &draw(&mut app, 120, 30));
    app.key(key(KeyCode::Char('G')));
    assert_modal_survives_resize(&mut app);
    assert!(app.pending.is_none());
}

#[test]
fn tui_manager_previews_are_private_responsive_and_fully_scrollable() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = manager_fixture(directory.path());
    let secret = "SYNTHETIC-private-file-must-not-be-previewed";
    let config = app.config.as_ref().unwrap();
    std::fs::write(&config.vault, secret).unwrap();
    for grant in &config.grants {
        std::fs::write(grant.client_file.as_ref().unwrap(), secret).unwrap();
    }
    let mut private = vec![secret.to_owned()];
    private.extend(
        config
            .connections
            .values()
            .map(|connection| connection.credential_id.clone()),
    );
    private.extend(config.grants.iter().flat_map(|grant| {
        [
            grant.capability_hash.clone(),
            grant.connection_fingerprint.clone(),
        ]
    }));
    private.extend(KEY_MATERIAL.iter().map(|value| (*value).to_owned()));
    app.webdav = Some(
        WebDavClient::new(
            WebDavProfile::new("https://dav.example.test/monica/", "monica-demo").unwrap(),
            Zeroizing::new(secret.to_owned()),
        )
        .unwrap(),
    );
    app.entries = vec![
        RemoteEntry {
            path: "archive".to_owned(),
            is_directory: true,
            size: None,
            etag: None,
        },
        RemoteEntry {
            path: "team.mdbx".to_owned(),
            is_directory: false,
            size: Some(524288),
            etag: Some("\"synthetic-v2\"".to_owned()),
        },
    ];
    for page in Page::ALL {
        app.set_page(page);
        if page == Page::Connections {
            app.key(key(KeyCode::Char('G')));
        }
        if page == Page::Grants {
            app.key(key(KeyCode::Char('j')));
        }
        if page == Page::WebDav {
            app.key(key(KeyCode::Char('j')));
        }
        for (width, height) in [
            (120, 34),
            (120, 30),
            (110, 20),
            (100, 24),
            (80, 24),
            (70, 20),
        ] {
            let buffer = draw(&mut app, width, height);
            let text = buffer
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            for value in &private {
                assert!(!text.contains(value));
            }
            // Wide glyphs have an empty continuation cell in the raw buffer.
            assert!(
                text.contains("monica://")
                    && text
                        .replace(' ', "")
                        .contains(tr!(app.language, ModeNormal))
            );
            assert_eq!(app.page_size, height as usize - 2);
            // Navigation and both rails stay present, even at 70/80 columns.
            let rails: Vec<_> = (0..width)
                .filter(|x| buffer[(*x, 1)].symbol() == "│")
                .collect();
            assert_eq!(rails.len(), 2);
            assert!(rails[0] >= width / 8 && rails[0] <= width / 8 + 1);
            assert!(rails[1] > width / 2 && rails[1] < width * 3 / 4);
            for y in 1..height - 1 {
                assert!(rails.iter().all(|x| buffer[(*x, y)].symbol() == "│"));
            }
            capture_buffer(
                &format!("page-{}-{width}x{height}", page.index() + 1),
                &buffer,
            );
        }
    }
    app.set_page(Page::Connections);
    filter(&mut app, "work-gitlab");
    app.config
        .as_mut()
        .unwrap()
        .connections
        .get_mut("work-gitlab")
        .unwrap()
        .note = "长用途说明含中文与 emoji 👩‍💻。".repeat(18);
    render(&mut app, 70, 20);
    app.key(key(KeyCode::Char('l')));
    assert!(app.focus == Focus::Preview);
    let selection = app.selected_connection();
    assert!(app.preview_max_scroll > 20);
    app.key(key(KeyCode::Char('G')));
    assert_eq!(app.preview_scroll, app.preview_max_scroll);
    let bottom = render(&mut app, 70, 20);
    assert!(bottom.contains("gitlab_create_issue"));
    app.key(key(KeyCode::Char('g')));
    assert_eq!(app.preview_scroll, 0);
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert!(app.preview_scroll > 0);
    app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert_eq!(app.preview_scroll, 0);
    assert_eq!(app.selected_connection(), selection);
    assert!(app.pending.is_none() && matches!(app.mode, Mode::Normal));
    app.key(key(KeyCode::Enter));
    assert!(app.focus == Focus::List && matches!(app.mode, Mode::Normal));
    assert_eq!(
        std::fs::read_to_string(&app.config.as_ref().unwrap().vault).unwrap(),
        secret
    );
    app.config.as_mut().unwrap().grants[2].operations =
        [crate::model::Operation::CreateIssue].into_iter().collect();
    app.set_page(Page::Grants);
    filter(&mut app, "仅写");
    assert_eq!(app.rows(), 1);
    assert_eq!(app.selected_grant().unwrap().name, "work-write");
    let write_only = render(&mut app, 120, 34).replace(' ', "");
    assert!(write_only.contains("仅写") && write_only.contains("gitlab_create_issue"));
    assert!(!write_only.contains("gitlab_list_issues") && !write_only.contains("gitlab_get_issue"));
}

#[test]
fn tui_help_and_results_overlay_preserve_selection_and_scroll_wrapped_lines() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = manager_fixture(directory.path());
    filter(&mut app, "work-gitlab");
    let selection = app.selection_key(Page::Connections);
    app.key(key(KeyCode::Char('?')));
    let help = draw(&mut app, 80, 24);
    capture_buffer("overlay-help-80x24", &help);
    assert!(matches!(&app.mode, Mode::Popup(popup) if popup.title.contains("快捷键")));
    app.key(key(KeyCode::Char('G')));
    assert!(
        matches!(&app.mode, Mode::Popup(popup) if popup.scroll == popup.max_scroll && popup.scroll > 0)
    );
    app.key(key(KeyCode::Esc));
    assert!(app.page == Page::Connections);
    assert_eq!(app.selection_key(Page::Connections), selection);
    assert_eq!(
        app.filters[Page::Connections.index()].as_str(),
        "work-gitlab"
    );

    // A single long error must scroll by rendered lines, not source line count.
    app.info(format!(
        "{} END-OF-MESSAGE",
        "合成结果包含中文与 👩‍💻，用于检查窄窗口。".repeat(80)
    ));
    app.failed = true;
    let normal = render(&mut app, 70, 20);
    assert!(normal.contains('!') && !normal.contains("END-OF-MESSAGE"));
    app.key(key(KeyCode::Char('!')));
    draw(&mut app, 70, 20);
    assert!(
        matches!(&app.mode, Mode::Popup(popup) if popup.lines.len() == 1 && popup.max_scroll > 20)
    );
    app.key(key(KeyCode::Char('G')));
    let bottom = render(&mut app, 70, 20);
    assert!(bottom.contains("END-OF-MESSAGE"));
    capture_buffer("overlay-result-70x20", &draw(&mut app, 70, 20));
    app.key(key(KeyCode::Char('g')));
    assert!(matches!(&app.mode, Mode::Popup(popup) if popup.scroll == 0));
    app.key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert!(matches!(&app.mode, Mode::Popup(popup) if popup.scroll > 0));
    app.key(key(KeyCode::Esc));
    assert_eq!(app.selection_key(Page::Connections), selection);
    app.key(key(KeyCode::Esc));
    assert!(app.message_at.is_none());

    app.invoke("icons");
    assert!(!app.nerd_font);
    let plain = draw(&mut app, 80, 24);
    assert!(
        !plain
            .content
            .iter()
            .flat_map(|cell| cell.symbol().chars())
            .any(|ch| ('\u{e000}'..='\u{f8ff}').contains(&ch))
    );
    capture_buffer("plain-80x24", &plain);
    assert!(app.pending.is_none() && !app.quitting);
}

#[test]
fn tui_vim_navigation_command_mode_and_password_fields_are_safe() {
    let directory = tempfile::tempdir().unwrap();
    let mut app = App::new(
        ConfigStore::new(directory.path().join("gateway.json")),
        Language::ZhCn,
    );
    app.key(key(KeyCode::F(3)));
    app.key(key(KeyCode::Char('5')));
    assert!(app.page == Page::Help);
    app.key(key(KeyCode::Char('G')));
    assert_eq!(app.selected[4], COMMANDS.len() - 1);
    app.key(key(KeyCode::Char('g')));
    app.key(key(KeyCode::Char(':')));
    for ch in "login".chars() {
        app.key(key(KeyCode::Char(ch)));
    }
    app.key(key(KeyCode::Enter));
    let Mode::Form(form) = &mut app.mode else {
        panic!("login form")
    };
    form.fields[0].input.insert("https://dav.example.test/");
    form.fields[1].input.insert("human");
    form.selected = 2;
    form.paste("synthetic-PRIVATE-password");
    for (width, height) in [(120, 30), (81, 25), (80, 24), (70, 20), (30, 10)] {
        let screen = render(&mut app, width, height);
        assert!(!screen.contains("synthetic-PRIVATE-password"));
        if width >= 70 {
            assert!(screen.contains("****"));
        }
        if width >= 70 {
            capture_buffer(
                &format!("overlay-login-{width}x{height}"),
                &draw(&mut app, width, height),
            );
        }
    }
    app.key(key(KeyCode::Esc));
    assert!(matches!(&app.mode, Mode::Form(form) if !form.insert));
    app.key(key(KeyCode::Char('k')));
    assert!(matches!(&app.mode, Mode::Form(form) if form.selected == 1));
    app.key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Normal));
    assert!(!app.store.path.exists());
}

#[test]
fn tui_unicode_editing_and_paste_do_not_execute_terminal_controls() {
    let mut input = Input::new("", 12);
    input.insert("保险库");
    input.key(key(KeyCode::Left));
    input.key(key(KeyCode::Backspace));
    assert_eq!(input.value.as_str(), "保库");
    input.insert("\u{001b}\r\n密");
    assert_eq!(input.value.as_str(), "保密库");
    input.insert("long paste beyond bound");
    assert!(input.value.len() <= 12);
    input.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    assert!(input.value.is_empty());
}

fn fill(app: &mut App, values: &[&str]) -> String {
    let Mode::Form(form) = &mut app.mode else {
        panic!("expected form")
    };
    assert_eq!(values.len(), form.fields.len());
    for (field, value) in form.fields.iter_mut().zip(values) {
        field
            .input
            .key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        field.input.insert(value);
    }
    form.selected = form.fields.len() - 1;
    form.authenticating = form.auth_field.is_some();
    render(app, 120, 38)
}

async fn settle(app: &mut App) {
    tokio::time::timeout(Duration::from_secs(60), async {
        while app.pending.is_some() {
            app.poll().await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!app.failed, "{}", app.message);
}

#[tokio::test]
async fn tui_quick_add_unlocks_and_exposes_only_public_named_metadata_to_mcp() {
    use crate::config::{ClientConfig, read_json};
    use crate::model::CONNECTION_CATALOG_TOOL;
    use crate::protocol::serve_mcp_io;
    use crate::test_support::{PASSWORD, REPOSITORY, TOKEN};
    use rmcp::model::CallToolRequestParams;
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
    use serde_json::json;

    let directory = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(directory.path().join("gateway.json"));
    let mut first_run = App::new(store.clone(), Language::ZhCn);
    first_run.key(key(KeyCode::F(3)));
    first_run.key(key(KeyCode::Char('c')));
    assert!(matches!(
        &first_run.mode,
        Mode::Form(Form {
            kind: Kind::Add { new_vault: true },
            ..
        })
    ));
    drop(first_run);
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    admin::initialize(
        &store,
        &directory.path().join("vault.mdbx"),
        port,
        PASSWORD,
        PASSWORD,
    )
    .unwrap();
    let mut app = App::new(store.clone(), Language::ZhCn);
    app.key(key(KeyCode::F(3)));
    app.key(key(KeyCode::Char('c')));
    let screen = fill(
        &mut app,
        &[
            "work",
            "工作 GitHub",
            "github",
            REPOSITORY,
            "处理产品问题反馈",
            "",
            TOKEN,
            PASSWORD,
        ],
    );
    assert!(!screen.contains(TOKEN));
    assert!(!screen.contains(PASSWORD));
    drop(reservation);
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app).await;
    assert!(app.broker.is_some());
    assert!(matches!(&app.mode, Mode::Popup(popup) if popup.title.contains("work")));
    let (_, client_path) = app.grant_client().unwrap();
    assert!(client_path.with_extension("mcp.json").is_file());
    let config: ClientConfig = read_json(&client_path, 16 * 1024).unwrap();
    let (server_io, client_io) = tokio::io::duplex(128 * 1024);
    let (read, write) = tokio::io::split(server_io);
    let bridge = tokio::spawn(serve_mcp_io(config, read, write));
    let client = ().serve_with_lifecycle(client_io, ClientLifecycleMode::Initialize).await.unwrap();
    let catalog = client
        .call_tool(CallToolRequestParams::new(CONNECTION_CATALOG_TOOL))
        .await
        .unwrap();
    let item = &catalog.structured_content.as_ref().unwrap()["connections"][0];
    assert_eq!(item["name"], "work");
    assert_eq!(item["note"], "处理产品问题反馈");
    assert_eq!(item["default_repository"], REPOSITORY);
    assert_eq!(item["tools"].as_array().unwrap().len(), 2);
    let denied = client.call_tool(CallToolRequestParams::new("github_create_issue").with_arguments(
        json!({"connection":"work", "title":"Not authorized", "request_id":uuid::Uuid::new_v4()}).as_object().unwrap().clone()
    )).await.unwrap();
    assert_eq!(
        denied.structured_content.as_ref().unwrap()["error"]["code"],
        "permission_denied"
    );
    let transcript = format!(
        "{}{}",
        serde_json::to_string(&catalog).unwrap(),
        serde_json::to_string(&denied).unwrap()
    );
    assert!(!transcript.contains(TOKEN));
    assert!(!transcript.contains(PASSWORD));
    client.cancel().await.unwrap();
    bridge.await.unwrap().unwrap();

    app.key(key(KeyCode::Esc));
    app.key(key(KeyCode::Char('2')));
    // Wide glyphs occupy an extra blank cell in TestBackend's flat buffer.
    assert!(
        render(&mut app, 120, 38)
            .replace(' ', "")
            .contains("处理产品问题反馈")
    );
    app.key(key(KeyCode::Char('e')));
    fill(&mut app, &["work", "更新后的 AI 用途说明", PASSWORD]);
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app).await;
    assert_eq!(
        store.load().unwrap().connections["work"].note,
        "更新后的 AI 用途说明"
    );
    assert!(app.broker.is_none());
    assert_eq!(store.load().unwrap().grants.len(), 1);
    app.key(key(KeyCode::Char('c')));
    fill(
        &mut app,
        &[
            "second",
            "",
            "github",
            REPOSITORY,
            "另一个只读连接",
            "",
            TOKEN,
            PASSWORD,
        ],
    );
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    // Quitting while setup is pending must also stop its newly started broker.
    app.key(key(KeyCode::Char('q')));
    app.finish().await.unwrap();
    assert_eq!(store.load().unwrap().grants.len(), 2);
    let probe = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await;
    assert!(probe.is_err());
}

/// The gate has to be reachable from the form a person actually uses, not only from
/// `monica grant`, so this issues three grants through the real TUI and reads each
/// stored policy back.
#[tokio::test]
async fn a_grant_issued_in_the_tui_carries_the_approval_gate() {
    use crate::model::{ApprovalPolicy, Provider};
    use crate::test_support::{PASSWORD, REPOSITORY, TOKEN};

    let directory = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(directory.path().join("gateway.json"));
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    admin::initialize(
        &store,
        &directory.path().join("vault.mdbx"),
        port,
        PASSWORD,
        PASSWORD,
    )
    .unwrap();
    drop(reservation);
    admin::add_connection(
        &store,
        "work",
        Provider::Github,
        "https://api.github.com/",
        "只读巡检",
        PASSWORD,
        Zeroizing::new(TOKEN.to_owned()),
    )
    .unwrap();
    let mut app = App::new(store.clone(), Language::ZhCn);
    for (name, approval, expected) in [
        ("plain", "", ApprovalPolicy::Off),
        ("writes", "write", ApprovalPolicy::Write),
        ("gated", "all", ApprovalPolicy::All),
    ] {
        app.key(key(KeyCode::Char(':')));
        for ch in "grant".chars() {
            app.key(key(KeyCode::Char(ch)));
        }
        app.key(key(KeyCode::Enter));
        assert!(
            matches!(&app.mode, Mode::Form(form) if matches!(form.kind, Kind::Grant) && form.fields.len() == 8),
            "{name} did not open the grant form"
        );
        let screen = fill(
            &mut app,
            &[
                name,
                "work",
                REPOSITORY,
                "list-issues,get-issue",
                "",
                "60",
                approval,
                PASSWORD,
            ],
        );
        assert!(!screen.contains(PASSWORD));
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        settle(&mut app).await;
        let config = store.load().unwrap();
        let stored = config
            .grants
            .iter()
            .find(|grant| grant.name == name)
            .unwrap_or_else(|| panic!("{name} was never issued"));
        assert_eq!(stored.approval, expected, "{name} kept no gate");
        app.key(key(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Normal));
    }

    // A policy the form cannot map is refused with the draft still on screen.
    app.key(key(KeyCode::Char(':')));
    for ch in "grant".chars() {
        app.key(key(KeyCode::Char(ch)));
    }
    app.key(key(KeyCode::Enter));
    fill(
        &mut app,
        &[
            "typo",
            "work",
            REPOSITORY,
            "list-issues,get-issue",
            "",
            "60",
            "sometimes",
            PASSWORD,
        ],
    );
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(app.failed);
    assert!(
        matches!(&app.mode, Mode::Form(_)),
        "a rejected gate must not close the form"
    );
    assert!(
        store
            .load()
            .unwrap()
            .grants
            .iter()
            .all(|grant| grant.name != "typo"),
        "a refused grant still wrote"
    );
    app.key(key(KeyCode::Esc));
    app.key(key(KeyCode::Char('q')));
    app.finish().await.unwrap();
}

/// Answering in the TUI is the whole approval mechanism, so this drives the real
/// broker: the call registers itself with the same queue the gateway parks on, and
/// the keystrokes have to reach that queue rather than the home key map.
#[tokio::test]
async fn tui_answers_a_waiting_call_with_y_and_n() {
    use crate::approval::{Decision, Request};
    use crate::test_support::{PASSWORD, REPOSITORY, TOKEN};
    use std::time::Instant;

    let directory = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(directory.path().join("gateway.json"));
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    admin::initialize(
        &store,
        &directory.path().join("vault.mdbx"),
        port,
        PASSWORD,
        PASSWORD,
    )
    .unwrap();
    let mut app = App::new(store.clone(), Language::ZhCn);
    app.key(key(KeyCode::F(3)));
    app.key(key(KeyCode::Char('c')));
    fill(
        &mut app,
        &[
            "work",
            "",
            "github",
            REPOSITORY,
            "只读巡检",
            "",
            TOKEN,
            PASSWORD,
        ],
    );
    drop(reservation);
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app).await;
    assert!(app.broker.is_some());
    app.key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Normal));

    let approvals = app.broker.as_ref().unwrap().approvals();
    let waiting = |tool: &str, is_write: bool| Request {
        id: 0,
        grant: "work".to_owned(),
        tool: tool.to_owned(),
        repository: REPOSITORY.to_owned(),
        is_write,
        preview: format!("{{\"title\":\"{tool} body\"}}"),
        asked_at: Instant::now(),
    };

    let refusal = approvals.ask("k1", waiting("github_create_issue", true));
    app.poll().await;
    assert!(app.approval.is_some(), "the prompt reached the screen");
    let screen = text(&mut app, 120, 38);
    assert!(screen.contains("github_create_issue"), "{screen}");
    assert!(screen.contains("github_create_issue body"), "{screen}");
    capture_buffer("approval-request-120x38", &draw(&mut app, 120, 38));
    // `n` would otherwise open the new-database form, and `y` would copy a field.
    app.key(key(KeyCode::Char('n')));
    assert_eq!(refusal.answer(), Some(Decision::Denied));
    assert!(app.approval.is_none());
    assert!(matches!(app.mode, Mode::Normal));

    let grant = approvals.ask("k2", waiting("github_list_issues", false));
    app.poll().await;
    assert_eq!(app.approval.as_ref().unwrap().tool, "github_list_issues");
    app.key(key(KeyCode::Char('y')));
    assert_eq!(grant.answer(), Some(Decision::Approved));
    app.poll().await;
    assert!(
        app.approval.is_none(),
        "an answered call leaves the screen on the next frame"
    );

    app.key(key(KeyCode::Char('q')));
    app.finish().await.unwrap();
}

/// This reaches the real OS credential manager, so it deletes exactly the entry it
/// creates. The fake server answers on an ephemeral port, so the target name can
/// never collide with a vault a person has actually logged into. The store is one
/// machine-wide resource, so the test takes the shared turn (`credential_turn`) that
/// keeps it from overlapping the other test writing real credentials.
/// Waiting for that turn across an await is the point: `#[tokio::test]` drives this body on
/// one thread, so the guard can never starve another task of this runtime.
#[cfg(windows)]
#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn a_typed_webdav_password_is_remembered_only_after_a_request_proves_it() {
    use crate::webdav::WebDavClient;
    use crate::webdav_tests::{DAV_PASSWORD, FakeWebDav};

    let _turn = crate::test_support::credential_turn()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let directory = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(directory.path().join("gateway.json"));
    let remote = FakeWebDav::new(Default::default()).await;
    let profile = remote.client.profile.clone();
    let account = (profile.base_url.as_str(), profile.username.as_str());
    credstore::forget(account.0, account.1).unwrap();

    let wrong = Zeroizing::new("definitely-not-the-app-password".to_owned());
    let client = WebDavClient::for_test(profile.clone(), &wrong, remote.server.client.clone());
    assert!(actions::login(&store, client, Some(wrong)).await.is_err());
    assert!(
        !credstore::present(account.0, account.1),
        "a password the server rejected must never be remembered"
    );

    let typed = Zeroizing::new(DAV_PASSWORD.to_owned());
    let client = WebDavClient::for_test(profile.clone(), &typed, remote.server.client.clone());
    let Outcome::Login { password_saved, .. } =
        actions::login(&store, client, Some(typed)).await.unwrap()
    else {
        unreachable!("login reports its own outcome")
    };
    assert!(password_saved, "a typed password must reach this computer");
    assert!(credstore::present(account.0, account.1));

    // The next sign-in reads it back instead of asking again.
    let stored = credstore::load(account.0, account.1).unwrap();
    let client = WebDavClient::for_test(profile.clone(), &stored, remote.server.client.clone());
    let Outcome::Login { password_saved, .. } = actions::login(&store, client, None).await.unwrap()
    else {
        unreachable!("login reports its own outcome")
    };
    assert!(password_saved, "an existing credential stays in force");

    assert!(credstore::forget(account.0, account.1).unwrap());
    assert!(!credstore::present(account.0, account.1));
}

#[tokio::test]
async fn tui_webdav_setup_to_mcp_call_revocation_and_quit_works_end_to_end() {
    use crate::config::{ClientConfig, read_json};
    use crate::model::Provider;
    use crate::protocol::serve_mcp_io;
    use crate::test_support::{FakeUpstream, REPOSITORY, Reply, TOKEN, issue};
    use crate::webdav_tests::{DAV_PASSWORD, FakeWebDav};
    use rmcp::model::CallToolRequestParams;
    use rmcp::service::{ClientLifecycleMode, ClientServiceExt};
    use serde_json::json;

    const PASSWORD: &str = "735941";
    let directory = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(directory.path().join("gateway.json"));
    let mut app = App::new(store.clone(), Language::ZhCn);
    app.key(key(KeyCode::F(3)));
    let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = reservation.local_addr().unwrap().port();
    let upstream = FakeUpstream::start(vec![Reply::json(issue(Provider::Github, 42))]).await;
    let remote = FakeWebDav::new(Default::default()).await;
    let mut screens = Vec::new();

    app.key(key(KeyCode::Char('n')));
    screens.push(fill(
        &mut app,
        &[
            &directory.path().join("vault.mdbx").display().to_string(),
            &port.to_string(),
            PASSWORD,
            PASSWORD,
        ],
    ));
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app).await;
    app.key(key(KeyCode::Char('C')));
    screens.push(fill(
        &mut app,
        &[
            "work",
            "工作令牌",
            "github",
            &format!("https://127.0.0.1:{}/", upstream.port),
            "用于项目 Issue 跟踪",
            TOKEN,
            PASSWORD,
        ],
    ));
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app).await;

    app.key(key(KeyCode::Char('i')));
    screens.push(fill(
        &mut app,
        &[&remote.client.profile.base_url, "human", DAV_PASSWORD],
    ));
    let Mode::Form(mut form) = std::mem::replace(&mut app.mode, Mode::Normal) else {
        panic!("login")
    };
    let Action::Login { profile, password } = form.action(&app).unwrap() else {
        panic!("login action")
    };
    let dav = WebDavClient::for_test(profile, &password, remote.server.client.clone());
    drop(password);
    // The test CA is the only substituted transport dependency; real HTTPS,
    // authentication, XML parsing, encrypted MDBX and UI actions are exercised.
    // `None` keeps this synthetic sign-in out of the real credential manager.
    app.apply(actions::login(&store, dav, None).await.unwrap());
    app.key(key(KeyCode::Char('P')));
    screens.push(fill(&mut app, &["vault.mdbx", PASSWORD]));
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app).await;
    let old_vault = store.load().unwrap().vault;
    app.key(key(KeyCode::Char('r')));
    settle(&mut app).await;
    assert_eq!(app.entries.len(), 1);
    app.key(key(KeyCode::Enter));
    screens.push(fill(&mut app, &[PASSWORD]));
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app).await;
    assert!(old_vault.is_file());
    assert_ne!(old_vault, store.load().unwrap().vault);
    assert_eq!(store.load().unwrap().connections.len(), 1);

    app.key(key(KeyCode::Char('a')));
    screens.push(fill(
        &mut app,
        &[
            "agent-read",
            "work",
            REPOSITORY,
            "list-issues,get-issue",
            "60",
            "60",
            "off",
            PASSWORD,
        ],
    ));
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app).await;
    screens.push(render(&mut app, 120, 38));
    let (_, client_path) = app.grant_client().unwrap();
    assert!(client_path.with_extension("mcp.json").is_file());
    app.key(key(KeyCode::Char('u')));
    screens.push(fill(&mut app, &[PASSWORD]));
    let Mode::Form(mut form) = std::mem::replace(&mut app.mode, Mode::Normal) else {
        panic!("unlock")
    };
    let Action::Unlock(password) = form.action(&app).unwrap() else {
        panic!("unlock action")
    };
    drop(reservation);
    app.apply(Outcome::Broker(
        BrokerSession::start_for_test(store.clone(), password, upstream.client.clone())
            .await
            .unwrap(),
    ));
    app.key(key(KeyCode::Char('p')));
    settle(&mut app).await;
    assert!(
        matches!(&app.mode, Mode::Popup(popup) if popup.lines.iter().any(|line| line.contains("github_get_issue")))
    );
    screens.push(render(&mut app, 120, 38));

    let config: ClientConfig = read_json(&client_path, 16 * 1024).unwrap();
    let capability = Zeroizing::new(config.capability.clone());
    let (server_io, client_io) = tokio::io::duplex(128 * 1024);
    let (read, write) = tokio::io::split(server_io);
    let bridge = tokio::spawn(serve_mcp_io(config, read, write));
    let client = ().serve_with_lifecycle(client_io, ClientLifecycleMode::Initialize).await.unwrap();
    let tools = client.list_tools(None).await.unwrap();
    assert_eq!(tools.tools.len(), 3);
    let call = |repository: &str| {
        CallToolRequestParams::new("github_get_issue").with_arguments(
            json!({"repository":repository,"number":42})
                .as_object()
                .unwrap()
                .clone(),
        )
    };
    let result = client.call_tool(call(REPOSITORY)).await.unwrap();
    assert_eq!(
        result.structured_content.as_ref().unwrap()["issue"]["number"],
        42
    );
    let denied = client.call_tool(call("outside/scope")).await.unwrap();
    assert_eq!(
        denied.structured_content.as_ref().unwrap()["error"]["code"],
        "permission_denied"
    );
    assert_eq!(
        upstream.requests()[0].headers["authorization"],
        format!("Bearer {TOKEN}")
    );
    app.key(key(KeyCode::Esc));
    app.key(key(KeyCode::Char('x')));
    app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    settle(&mut app).await;
    assert!(app.broker.is_some());
    let revoked = client.call_tool(call(REPOSITORY)).await.unwrap();
    assert_eq!(
        revoked.structured_content.as_ref().unwrap()["error"]["code"],
        "unauthorized"
    );
    assert_eq!(upstream.requests().len(), 1);
    let transcript = format!(
        "{}{}{}{}{}",
        screens.join("\n"),
        serde_json::to_string(&tools).unwrap(),
        serde_json::to_string(&result).unwrap(),
        serde_json::to_string(&denied).unwrap(),
        serde_json::to_string(&revoked).unwrap()
    );
    for secret in [TOKEN, PASSWORD, DAV_PASSWORD, capability.as_str()] {
        assert!(!transcript.contains(secret));
    }
    for path in [
        &store.path,
        &store.path.with_extension("webdav.json"),
        &client_path.with_extension("mcp.json"),
    ] {
        let bytes = std::fs::read(path).unwrap();
        for secret in [TOKEN, PASSWORD, DAV_PASSWORD] {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|bytes| bytes == secret.as_bytes())
            );
        }
    }
    app.key(key(KeyCode::Char('q')));
    app.poll().await;
    settle(&mut app).await;
    assert!(app.broker.is_none());
    assert!(store.acquire_broker_lock().is_ok());
    client.cancel().await.unwrap();
    bridge.await.unwrap().unwrap();
    app.finish().await.unwrap();
}
