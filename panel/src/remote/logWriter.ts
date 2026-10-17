/**
 * Who saves a conversation's panel log (`session_update_log`).
 *
 * The panel log is the conversation the user sees and the transcript a reset
 * or rewound conversation continues from, and each client builds it from the
 * events it received plus the rows only it wrote (its own messages and
 * notes). The app keeps the last write. A phone that only watches would
 * overwrite a PC window's log with one that lacks the PC's own rows, so it
 * saves only a conversation it took part in, or one no PC window is showing
 * (then nobody else saves the events it receives). A PC main window saves
 * every conversation of its project, shown in a pane or not, so a phone
 * watching a conversation the PC has open in another pane still leaves the
 * saving to the PC.
 */
import type { InvokeFn, PanelLog } from "@/lib/ipc";
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

/**
 * Whether the saved log is newer than `ours`: a save the phone scheduled
 * before it lost the connection would overwrite rows the PC wrote since.
 * `logSeq` is the log's high-water row id, which every client carries on
 * from the saved log it loaded.
 */
export function savedLogIsNewer(saved: PanelLog | undefined | null, ours: PanelLog): boolean {
  return (saved?.logSeq ?? 0) > ours.logSeq;
}

export interface LogSaver {
  /** The log changed: save it after a pause, and at least every `maxWaitMs`. */
  schedule(): void;
  /** Save now what is pending, if anything. */
  flush(): Promise<void>;
  /** Forget what is pending. */
  cancel(): void;
  /** Whether a save is pending. */
  pending(): boolean;
}

/**
 * A debounced save that a stream of changes cannot postpone forever: a long
 * streaming turn changes the log many times a second, and is still saved at
 * least every `maxWaitMs`.
 */
export function createLogSaver(
  save: () => Promise<void>,
  delayMs = 500,
  maxWaitMs = 3_000,
): LogSaver {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let firstPendingAt: number | undefined;
  const run = async () => {
    timer = undefined;
    firstPendingAt = undefined;
    await save();
  };
  return {
    schedule() {
      const now = Date.now();
      firstPendingAt ??= now;
      if (timer !== undefined) clearTimeout(timer);
      const wait = Math.max(0, Math.min(delayMs, firstPendingAt + maxWaitMs - now));
      timer = setTimeout(() => void run(), wait);
    },
    async flush() {
      if (timer === undefined) return;
      clearTimeout(timer);
      await run();
    },
    cancel() {
      if (timer !== undefined) clearTimeout(timer);
      timer = undefined;
      firstPendingAt = undefined;
    },
    pending() {
      return timer !== undefined;
    },
  };
}
