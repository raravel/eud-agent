/** True inside the macOS WKWebView host; Windows WebView2 and tests report false. */
export function isMacOS(): boolean {
  return typeof navigator !== "undefined" && /Mac OS X|Macintosh/.test(navigator.userAgent);
}

/** File name of the frozen euddraft launcher shipped for the host platform. */
export function euddraftExecutableName(): string {
  return isMacOS() ? "euddraft" : "euddraft.exe";
}
