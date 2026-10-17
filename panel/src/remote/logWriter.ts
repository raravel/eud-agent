/**
 * Who saves a conversation's panel log (`session_update_log`).
 *
 * The panel log is the conversation the user sees and the transcript a reset
 * or rewound conversation continues from, and each client builds it from the
 * events it received plus the rows only it wrote (its own messages and
 * notes). The app keeps the last write. A phone that only watches would
 * overwrite a PC window's log with one that lacks the PC's own rows, so it
 * saves only a conversation it took part in, or one no PC window is showing
 * (then nobody else saves the events it receives).
 */
import type { InvokeFn } from "@/lib/ipc";
import type { RemoteProjectEntry } from "@/lib/remoteTransport";

/** Whether no PC window shows project `projectId` (false when unknown). */
export async function projectHasNoDesktopWindow(
  invoke: InvokeFn,
  projectId: string | null,
): Promise<boolean> {
  if (projectId === null) return false;
  try {
    const entries = (await invoke("remote_projects")) as RemoteProjectEntry[];
    const entry = entries.find((candidate) => candidate.projectId === projectId);
    return entry !== undefined && entry.open && !entry.desktopWindow;
  } catch {
    return false;
  }
}

/** Save the log when this phone changed the conversation, or nobody else can. */
export async function maySaveLog(
  participated: boolean,
  invoke: InvokeFn,
  projectId: string | null,
): Promise<boolean> {
  return participated || projectHasNoDesktopWindow(invoke, projectId);
}
