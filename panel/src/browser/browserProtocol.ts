import { invoke } from "@/lib/transport";

/** One tab of the app's browser window, as the chrome strip shows it. */
export interface BrowserTab {
  tabId: string;
  sessionId: string;
  sessionTitle: string;
  url: string;
  title: string;
  active: boolean;
  /** An agent tool call is running on this tab. */
  agentBusy: boolean;
  /** An agent action on this tab waits for the user's 허용/거부 in the chat. */
  awaitingConfirmation: boolean;
}

export type BrowserDownloadState = "inProgress" | "completed" | "failed";

/** One file a tab downloaded (an agent call's or the user's own). */
export interface BrowserDownload {
  downloadId: string;
  tabId: string;
  sessionId: string;
  sessionTitle: string;
  fileName: string;
  bytes: number;
  state: BrowserDownloadState;
  error: string | null;
}

export interface BrowserState {
  tabs: BrowserTab[];
  activeTabId: string | null;
  /** Every download, newest first. */
  downloads: BrowserDownload[];
}

/** Emitted to the browser window whenever a tab changes. */
export const BROWSER_STATE_EVENT = "browser-state-changed";

export type BrowserAction = "back" | "forward" | "reload";

export function browserState(): Promise<BrowserState> {
  return invoke<BrowserState>("browser_state");
}

export function browserActivate(tabId: string): Promise<void> {
  return invoke("browser_activate", { tabId });
}

export function browserTabClose(tabId: string): Promise<void> {
  return invoke("browser_tab_close", { tabId });
}

/** The user's own navigation: a typed address, or back / forward / reload. */
export function browserUserNavigate(
  tabId: string,
  target: { url: string } | { action: BrowserAction },
): Promise<void> {
  return invoke("browser_user_navigate", { tabId, ...target });
}

/**
 * The downloads list is open: the chrome covers the page area so the list is
 * not cut off (`false` gives the area back to the page).
 */
export function browserChromeExpand(expanded: boolean): Promise<void> {
  return invoke("browser_chrome_expand", { expanded });
}
