/**
 * One project HTML file rendered in the document viewer.
 *
 * The page comes from the app's `eudpreview` protocol (`html_preview.rs`), so
 * its relative assets resolve inside the project. The frame is sandboxed
 * without `allow-same-origin`: the page runs scripts but has an opaque origin
 * and reaches no app command. Links that leave the project arrive as a message
 * from the script the protocol appends, and open in the system browser only
 * right after the user's own click.
 */
import { convertFileSrc } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";

/** Message type of the script `html_preview.rs` appends to every page. */
const LINK_MESSAGE = "eud-preview-link";

/** At most one link opens in this time, whatever a page posts. */
const LINK_INTERVAL_MS = 1000;

const DOCUMENT_FOLDER = ".eud-agent/workspace/";

/** An HTML file the preview protocol serves (hidden folders stay source only). */
export function isPreviewableHtml(path: string): boolean {
  if (!/\.html?$/i.test(path)) return false;
  if (path.startsWith(DOCUMENT_FOLDER)) return true;
  return !path.split("/").some((segment) => segment.startsWith("."));
}

/** The protocol URL of a project path; its slashes stay so relative links work. */
export function previewUrl(path: string): string {
  return (
    convertFileSrc("", "eudpreview") +
    path.split("/").map(encodeURIComponent).join("/")
  );
}

export interface HtmlPreviewProps {
  path: string;
  title: string;
  onOpenLink(url: string): void;
}

export function HtmlPreview({ path, title, onOpenLink }: HtmlPreviewProps) {
  const frameRef = useRef<HTMLIFrameElement | null>(null);
  const lastOpenRef = useRef(-Infinity);

  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      const frame = frameRef.current?.contentWindow;
      if (!frame || event.source !== frame) return;
      const data: unknown = event.data;
      if (
        typeof data !== "object" ||
        data === null ||
        !("type" in data) ||
        data.type !== LINK_MESSAGE ||
        !("href" in data) ||
        typeof data.href !== "string" ||
        !/^https?:\/\//i.test(data.href)
      ) {
        return;
      }
      // A page script can post this too; only a user's click opens a browser.
      // The click that opened this tab left the focus outside the frame.
      if (!navigator.userActivation?.isActive) return;
      if (document.activeElement !== frameRef.current) return;
      // One click keeps the activation for seconds.
      if (event.timeStamp - lastOpenRef.current < LINK_INTERVAL_MS) return;
      lastOpenRef.current = event.timeStamp;
      onOpenLink(data.href);
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [onOpenLink]);

  return (
    <iframe
      ref={frameRef}
      src={previewUrl(path)}
      title={title}
      sandbox="allow-scripts allow-forms"
      // Pages assume a browser's white canvas, not the panel's dark one.
      className="size-full border-0 bg-white scheme-light"
    />
  );
}
