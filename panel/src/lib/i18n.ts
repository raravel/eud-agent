/**
 * The panel's interface language (Korean or English).
 *
 * The app owns the choice (`ui_language_get`/`ui_language_set`, stored in its
 * config; until the user picks one it follows the OS). Every window loads it
 * before its first render and follows `ui-language-changed`, so a change in
 * Settings reaches every open window at once. Text lives in
 * `locales/{ko,en}.json`; a key missing in one language shows the Korean text.
 */
import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import en from "@/locales/en.json";
import ko from "@/locales/ko.json";

import { invoke, isRemote } from "./transport";
import { listenToCurrentWindow } from "./windowEvents";

export type UiLanguage = "ko" | "en";

export interface UiLanguageState {
  language: UiLanguage;
  /** `true` when the user chose it, `false` when it follows the OS. */
  chosen: boolean;
}

export const UI_LANGUAGE_CHANGED_EVENT = "ui-language-changed";

/** Korean for a `ko`/`ko-KR` locale, English for any other. */
export function languageFromLocale(locale: string | undefined): UiLanguage {
  const primary = (locale ?? "").split(/[-_.]/)[0]?.toLowerCase();
  return primary === "ko" ? "ko" : "en";
}

function browserLanguage(): UiLanguage {
  if (typeof navigator === "undefined") return "ko";
  return languageFromLocale(navigator.language);
}

function applyDocumentLanguage(language: UiLanguage): void {
  if (typeof document !== "undefined") document.documentElement.lang = language;
}

void i18n.use(initReactI18next).init({
  resources: { ko: { translation: ko }, en: { translation: en } },
  lng: "ko",
  fallbackLng: "ko",
  interpolation: { escapeValue: false },
  returnNull: false,
  initAsync: false,
  missingKeyHandler: import.meta.env.DEV
    ? (languages, _namespace, key) =>
        console.warn(`[i18n] missing key "${key}" for ${languages.join(",")}`)
    : undefined,
  saveMissing: import.meta.env.DEV,
});

export function setPanelLanguage(language: UiLanguage): void {
  applyDocumentLanguage(language);
  if (i18n.language !== language) void i18n.changeLanguage(language);
}

/**
 * Load the app's language and follow its changes. Resolves before the first
 * render so no window flashes the other language. Without the app (the phone
 * before pairing, a failed call) the browser's language decides.
 */
export async function initPanelLanguage(): Promise<void> {
  // The phone page asks the app only once it is paired (see RemoteApp); until
  // then its own browser language decides.
  if (isRemote()) {
    setPanelLanguage(browserLanguage());
    return;
  }
  try {
    const state = await invoke<UiLanguageState>("ui_language_get");
    setPanelLanguage(state.language);
  } catch {
    setPanelLanguage(browserLanguage());
  }
  try {
    await listenToCurrentWindow<UiLanguageState>(UI_LANGUAGE_CHANGED_EVENT, (event) => {
      setPanelLanguage(event.payload.language);
    });
  } catch {
    // No event channel (unpaired phone): the language stays as loaded.
  }
}

/** Choose the language (`null` follows the OS). Every window follows the event. */
export async function saveUiLanguage(language: UiLanguage | null): Promise<UiLanguageState> {
  const state = await invoke<UiLanguageState>("ui_language_set", { language });
  setPanelLanguage(state.language);
  return state;
}

export function loadUiLanguage(): Promise<UiLanguageState> {
  return invoke<UiLanguageState>("ui_language_get");
}

export default i18n;
