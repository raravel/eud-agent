//! The app's user interface language (Korean or English).
//!
//! The chosen language is stored in `config.json` (`ui_language`). Until the user picks one, the
//! OS language decides: Korean for a Korean OS, English for every other. The process keeps the
//! effective language in one global so user-facing Rust messages (`rust_i18n::t!`) and the
//! model's language note follow it without threading it through every call. Text only the model
//! reads stays as it is.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use crate::config::DataDirs;
use crate::ipc::AppManaged;

/// Event every window receives when the language changes.
/// Every locale file, read here so cargo rebuilds when one changes (`rust_i18n::i18n!`
/// loads the folder at expansion time and does not track it). A test checks that this
/// list names every file in `locales/`.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const LOCALE_FILES: &[(&str, &str)] = &[
    ("app.yml", include_str!("../locales/app.yml")),
    ("browser.yml", include_str!("../locales/browser.yml")),
    ("engine.yml", include_str!("../locales/engine.yml")),
    ("map.yml", include_str!("../locales/map.yml")),
    ("project.yml", include_str!("../locales/project.yml")),
    ("remote.yml", include_str!("../locales/remote.yml")),
];

pub const UI_LANGUAGE_CHANGED_EVENT: &str = "ui-language-changed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UiLanguage {
    #[serde(rename = "ko")]
    Korean,
    #[serde(rename = "en")]
    English,
}

impl UiLanguage {
    pub fn code(self) -> &'static str {
        match self {
            Self::Korean => "ko",
            Self::English => "en",
        }
    }

    /// Korean for a `ko`/`ko-KR`/`ko_KR` locale, English for any other.
    pub fn from_locale(locale: &str) -> Self {
        let primary = locale
            .split(['-', '_', '.'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if primary == "ko" {
            Self::Korean
        } else {
            Self::English
        }
    }

    /// The OS user interface language; English when it cannot be read.
    pub fn from_os() -> Self {
        sys_locale::get_locale()
            .map(|locale| Self::from_locale(&locale))
            .unwrap_or(Self::English)
    }
}

/// Translate a `locales/app.yml` key into the current language: `tr!("tray.open")`,
/// `tr!("tray.status", target = name)`. Always passes the language explicitly, so text never
/// depends on `rust_i18n`'s own global locale.
#[macro_export]
macro_rules! tr {
    ($key:expr) => {
        rust_i18n::t!($key, locale = $crate::i18n::current().code()).into_owned()
    };
    ($key:expr, $($name:ident = $value:expr),+ $(,)?) => {
        rust_i18n::t!($key, locale = $crate::i18n::current().code(), $($name = $value),+).into_owned()
    };
}

const KOREAN: u8 = 0;
const ENGLISH: u8 = 1;

static CURRENT: AtomicU8 = AtomicU8::new(KOREAN);

/// The language the app currently speaks.
pub fn current() -> UiLanguage {
    match CURRENT.load(Ordering::Relaxed) {
        ENGLISH => UiLanguage::English,
        _ => UiLanguage::Korean,
    }
}

/// Make `language` the app's language for every later message.
pub fn set_current(language: UiLanguage) {
    CURRENT.store(
        match language {
            UiLanguage::Korean => KOREAN,
            UiLanguage::English => ENGLISH,
        },
        Ordering::Relaxed,
    );
}

/// The stored choice, or the OS language when nothing was chosen.
pub fn effective(stored: Option<UiLanguage>) -> UiLanguage {
    stored.unwrap_or_else(UiLanguage::from_os)
}

/// Apply the stored language at start; a config that cannot be read falls back to the OS.
pub fn init_from_config(dirs: &DataDirs) {
    let stored = dirs
        .load_config()
        .ok()
        .and_then(|config| config.ui_language);
    set_current(effective(stored));
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiLanguageState {
    /// The language in effect.
    pub language: UiLanguage,
    /// `true` when the user chose it, `false` when it follows the OS.
    pub chosen: bool,
}

pub fn language_state(dirs: &DataDirs) -> Result<UiLanguageState, String> {
    let config = dirs.load_config().map_err(|error| error.to_string())?;
    Ok(UiLanguageState {
        language: effective(config.ui_language),
        chosen: config.ui_language.is_some(),
    })
}

/// Store `language` (`None` = follow the OS) and make it current.
pub fn save_language(
    dirs: &DataDirs,
    language: Option<UiLanguage>,
) -> Result<UiLanguageState, String> {
    let _guard = crate::config::PROJECT_CONFIG_LOCK.lock();
    let mut config = dirs.load_config().map_err(|error| error.to_string())?;
    config.ui_language = language;
    dirs.save_config(&config)
        .map_err(|error| error.to_string())?;
    let state = UiLanguageState {
        language: effective(language),
        chosen: language.is_some(),
    };
    set_current(state.language);
    Ok(state)
}

/// The app's language and whether the user chose it.
#[tauri::command]
pub async fn ui_language_get(
    state: tauri::State<'_, AppManaged>,
) -> Result<UiLanguageState, String> {
    language_state(state.dirs())
}

/// Choose the app's language (`null` follows the OS) and tell every window.
#[tauri::command]
pub async fn ui_language_set(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppManaged>,
    language: Option<UiLanguage>,
) -> Result<UiLanguageState, String> {
    let saved = save_language(state.dirs(), language)?;
    // Not project-owned: every window, whatever its project, shows the new language.
    let _ = app.emit(UI_LANGUAGE_CHANGED_EVENT, &saved);
    // A phone has no window: its event socket carries the change.
    if let Some(hub) = app.try_state::<crate::remote::hub::RemoteHub>() {
        hub.publish_global(UI_LANGUAGE_CHANGED_EVENT, &saved);
    }
    crate::remote::tray::sync(&app).await;
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_primary_tag_decides_korean() {
        for locale in ["ko", "ko-KR", "ko_KR", "KO-kr", "ko_KR.UTF-8"] {
            assert_eq!(
                UiLanguage::from_locale(locale),
                UiLanguage::Korean,
                "{locale}"
            );
        }
        for locale in ["en-US", "ja-JP", "", "kok-IN", "zh_CN"] {
            assert_eq!(
                UiLanguage::from_locale(locale),
                UiLanguage::English,
                "{locale}"
            );
        }
    }

    #[test]
    fn a_stored_choice_wins_over_the_os() {
        assert_eq!(effective(Some(UiLanguage::Korean)), UiLanguage::Korean);
        assert_eq!(effective(Some(UiLanguage::English)), UiLanguage::English);
    }

    /// `key -> [(language, text)]` of one locale file in the `_version: 2` layout.
    fn parse_locale_file(source: &str) -> Vec<(String, Vec<(String, String)>)> {
        let mut keys: Vec<(String, Vec<(String, String)>)> = Vec::new();
        for line in source.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || line.starts_with("_version") {
                continue;
            }
            if !line.starts_with(' ') {
                keys.push((
                    line.trim_end_matches(':').trim_matches('"').to_string(),
                    Vec::new(),
                ));
            } else if let Some((_, languages)) = keys.last_mut() {
                let (language, text) = trimmed.split_once(':').unwrap_or((trimmed, ""));
                languages.push((language.to_string(), text.trim().to_string()));
            }
        }
        keys
    }

    fn placeholders(text: &str) -> Vec<String> {
        let mut out: Vec<String> = text
            .split("%{")
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name.to_string()))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn every_locale_file_is_listed() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();
        let mut listed: Vec<String> = LOCALE_FILES
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        listed.sort();
        assert_eq!(listed, on_disk);
    }

    #[test]
    fn every_message_key_carries_both_languages_with_the_same_placeholders() {
        let mut seen = std::collections::BTreeSet::new();
        for (file, source) in LOCALE_FILES {
            for (key, mut languages) in parse_locale_file(source) {
                assert!(seen.insert(key.clone()), "{file}: {key} is defined twice");
                languages.sort();
                let names: Vec<&str> = languages
                    .iter()
                    .map(|(language, _)| language.as_str())
                    .collect();
                assert_eq!(names, ["en", "ko"], "{file}: {key}");
                assert_eq!(
                    placeholders(&languages[0].1),
                    placeholders(&languages[1].1),
                    "{file}: {key} placeholders differ"
                );
                let hangul = languages[0].1.chars().any(|c| ('가'..='힣').contains(&c));
                assert!(!hangul, "{file}: {key} English text has Hangul");
            }
        }
        assert!(!seen.is_empty());
    }

    #[test]
    fn messages_follow_the_current_language() {
        // The process default is Korean; nothing in the test binary switches it.
        assert_eq!(crate::tr!("tray.open"), "창 열기");
        assert_eq!(
            rust_i18n::t!("tray.status", locale = "en", target = "pc").into_owned(),
            "Remote access: on · pc"
        );
    }

    #[test]
    fn language_serializes_as_its_code() {
        assert_eq!(
            serde_json::to_string(&UiLanguage::English).unwrap(),
            "\"en\""
        );
        assert_eq!(
            serde_json::from_str::<UiLanguage>("\"ko\"").unwrap(),
            UiLanguage::Korean
        );
    }
}
