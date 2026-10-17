/**
 * The durable form of a conversation log (`SessionRecord.panelLog`), shared
 * by the desktop panel and the remote page.
 */
import type { PanelLog, PanelLogEntry } from "@/lib/ipc";
import type { LogEntry } from "@/state/store";

/** panelLog schema version (features/sessions.md ## panelLog schema). */
export const PANEL_LOG_SCHEMA_VERSION = 2;

/**
 * Serialize the live conversation log into the durable {@link PanelLog} subset.
 * Turn progress is transient; durable rows keep `id/kind/text` plus optional
 * `stage`/`tools`/`attachments`/`mentions`. `logSeq` advances restored store
 * counters past every live id, including omitted progress rows.
 */
export function serializePanelLog(log: readonly LogEntry[]): PanelLog {
  const entries: PanelLogEntry[] = [];
  for (const entry of log) {
    if (entry.kind === "progress") continue;
    const next: PanelLogEntry = {
      id: entry.id,
      kind: entry.kind,
      text: entry.text,
    };
    if (entry.clientTurnId) next.clientTurnId = entry.clientTurnId;
    if (entry.stage) next.stage = entry.stage;
    if (entry.tools) {
      next.tools = entry.tools.map((tool) => ({
        id: tool.id,
        name: tool.name,
        state: tool.state,
        ...(tool.args !== undefined ? { args: tool.args } : {}),
        ...(tool.detail !== undefined ? { detail: tool.detail } : {}),
      }));
    }
    if (entry.attachments) {
      next.attachments = entry.attachments.map((attachment) => ({
        id: attachment.id,
        name: attachment.name,
        mime: attachment.mime,
        kind: attachment.kind,
        size: attachment.size,
        ...(attachment.kind === "image" &&
        attachment.previewUrl?.startsWith("data:image/") === true
          ? { previewUrl: attachment.previewUrl }
          : {}),
      }));
    }
    if (entry.mentions) {
      next.mentions = entry.mentions.map((mention) => ({ ...mention }));
    }
    if (entry.detail) next.detail = entry.detail;
    entries.push(next);
  }
  const logSeq = log.reduce((max, entry) => (entry.id > max ? entry.id : max), 0);
  return { schemaVersion: PANEL_LOG_SCHEMA_VERSION, logSeq, log: entries };
}

export function emptyPanelLog(): PanelLog {
  return { schemaVersion: PANEL_LOG_SCHEMA_VERSION, logSeq: 0, log: [] };
}
