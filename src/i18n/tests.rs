use super::*;

#[test]
fn locale_resolution_obeys_explicit_environment_saved_and_system_precedence() {
    use LanguageChoice::{Auto, En, ZhCn};
    for (explicit, environment, saved, system, expected) in [
        (None, None, Auto, Language::ZhCn, Language::ZhCn),
        (None, None, En, Language::ZhCn, Language::En),
        (None, Some(ZhCn), En, Language::En, Language::ZhCn),
        (Some(En), Some(ZhCn), ZhCn, Language::ZhCn, Language::En),
        (Some(Auto), Some(En), En, Language::ZhCn, Language::ZhCn),
        (None, Some(Auto), ZhCn, Language::En, Language::En),
    ] {
        assert_eq!(resolve(explicit, environment, saved, system), expected);
    }
    for locale in ["zh-CN", "zh_CN.UTF-8", "ZH_hans_CN", "zh_TW"] {
        assert_eq!(Language::from_locale(locale), Some(Language::ZhCn));
    }
    for locale in ["en-US", "en_GB.UTF-8", "C", "C.UTF-8", "POSIX"] {
        assert_eq!(Language::from_locale(locale), Some(Language::En));
    }
    assert_eq!(Language::from_locale("fr-FR"), None);
    assert_eq!(LanguageChoice::parse("zh-cn"), Some(ZhCn));
    assert_eq!(LanguageChoice::parse("unsupported"), None);
}

#[test]
fn preferences_are_private_bounded_and_independent_of_vault_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let store = ConfigStore::new(directory.path().join("gateway.json"));
    assert_eq!(
        Preferences::load(&store).unwrap().language,
        LanguageChoice::Auto
    );
    Preferences {
        language: LanguageChoice::ZhCn,
    }
    .save(&store)
    .unwrap();
    assert_eq!(
        Preferences::load(&store).unwrap().language,
        LanguageChoice::ZhCn
    );
    assert!(!store.path.exists());
    assert!(!directory.path().join("gateway.mdbx").exists());
    let path = store.path.with_extension("preferences.json");
    std::fs::write(&path, r#"{"language":"en","token":"unsupported"}"#).unwrap();
    assert!(Preferences::load(&store).is_err());
    std::fs::write(&path, " ".repeat(4097)).unwrap();
    assert!(Preferences::load(&store).is_err());
}

#[test]
fn all_catalog_entries_have_matching_named_placeholders() {
    fn placeholders(text: &str) -> std::collections::BTreeSet<&str> {
        text.split('{')
            .skip(1)
            .map(|part| part.split_once('}').expect("unclosed placeholder").0)
            .collect()
    }
    for message in Message::ALL {
        let en = Language::En.text(*message);
        let zh = Language::ZhCn.text(*message);
        assert!(!en.is_empty() && !zh.is_empty(), "{message:?}");
        assert_eq!(placeholders(en), placeholders(zh), "{message:?}");
        assert!(!en.contains(['\u{1b}', '\r']), "{message:?}");
        assert!(!zh.contains(['\u{1b}', '\r']), "{message:?}");
    }
}

#[test]
fn user_values_are_never_interpreted_as_translation_templates() {
    for language in [Language::En, Language::ZhCn] {
        let value = "{name} {language} 中文 👩‍💻";
        let result = crate::tr!(language, LanguageSaved, language = value);
        assert!(result.contains(value));
        assert_eq!(result.matches(value).count(), 1);
    }
}

#[test]
fn human_localization_preserves_machine_error_codes_and_messages() {
    let error = GatewayError::PermissionDenied;
    let original = error.response();
    assert!(Language::ZhCn.error(error).contains("不允许"));
    assert_eq!(Language::En.error(error), error.to_string());
    assert_eq!(error.response(), original);
    assert_eq!(original["error"]["code"], "permission_denied");
}
