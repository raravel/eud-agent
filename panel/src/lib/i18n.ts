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

type LocaleTree = { [key: string]: string | LocaleTree };

/**
 * `locales/<lang>/<area>.json`, merged under the file name: `settings.json`
 * holds the keys `settings.*`. One file per panel area, so areas are
 * translated independently.
 */
function loadLocale(files: Record<string, unknown>): LocaleTree {
  const tree: LocaleTree = {};
  for (const [path, module] of Object.entries(files)) {
    const area = path.split("/").pop()!.replace(/\.json$/, "");
    tree[area] = ((module as { default?: LocaleTree }).default ?? module) as LocaleTree;
  }
  return tree;
}

export const localeTrees: Record<UiLanguage, LocaleTree> = {
  ko: loadLocale(import.meta.glob("@/locales/ko/*.json", { eager: true })),
  en: loadLocale(import.meta.glob("@/locales/en/*.json", { eager: true })),
};
const { ko, en } = localeTrees;

// Loaded on first use, not at import: tests import this module from their
// setup file, and a static import would load the real transport before a
// test's `vi.mock("@/lib/transport")` could replace it.
const transport = () => import("./transport");
const windowEvents = () => import("./windowEvents");

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
/** The BCP 47 locale `Intl` formats dates and numbers in for the current panel language. */
export function intlLocale(): string {
  return i18n.language === "en" ? "en-US" : "ko-KR";
}

export async function initPanelLanguage(): Promise<void> {
  // The phone page asks the app only once it is paired (see RemoteApp); until
  // then its own browser language decides.
  const { invoke, isRemote } = await transport();
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
    const { listenToCurrentWindow } = await windowEvents();
    await listenToCurrentWindow<UiLanguageState>(UI_LANGUAGE_CHANGED_EVENT, (event) => {
      setPanelLanguage(event.payload.language);
    });
  } catch {
    // No event channel (unpaired phone): the language stays as loaded.
  }
}

/** Choose the language (`null` follows the OS). Every window follows the event. */
export async function saveUiLanguage(language: UiLanguage | null): Promise<UiLanguageState> {
  const { invoke } = await transport();
  const state = await invoke<UiLanguageState>("ui_language_set", { language });
  setPanelLanguage(state.language);
  return state;
}

export async function loadUiLanguage(): Promise<UiLanguageState> {
  const { invoke } = await transport();
  return invoke<UiLanguageState>("ui_language_get");
}

export default i18n;
