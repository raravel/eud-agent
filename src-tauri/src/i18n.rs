//! The app's user interface language (Korean or English).
//!
//! The chosen language is stored in `config.json` (`ui_language`). Until the user picks one, the
//! OS language decides: Korean for a Korean OS, English for every other. The process keeps the
//! effective language in one global so user-facing Rust messages (`rust_i18n::t!`) and the
//! model's language note follow it without threading it through every call. Text only the model
//! reads stays as it is.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::config::DataDirs;
use crate::ipc::AppManaged;

/// Event every window receives when the language changes.
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

    #[test]
    fn every_message_key_carries_both_languages() {
        let source = include_str!("../locales/app.yml");
        let mut keys: Vec<(String, Vec<String>)> = Vec::new();
        for line in source.lines() {
            if line.trim().is_empty() || line.trim_start().starts_with('#') {
                continue;
            }
            if !line.starts_with(' ') {
                let key = line.trim_end_matches(':').to_string();
                if key.starts_with("_version") {
                    continue;
                }
                keys.push((key, Vec::new()));
            } else if let Some((_, languages)) = keys.last_mut() {
                let language = line.trim().split(':').next().unwrap_or_default();
                languages.push(language.to_string());
            }
        }
        assert!(!keys.is_empty());
        for (key, mut languages) in keys {
            languages.sort();
            assert_eq!(languages, ["en", "ko"], "{key}");
        }
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
