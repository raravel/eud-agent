/**
 * What the phone's project screen shows and does for each row: open
 * projects (attached to) and recent ones (opened on the PC first).
 */
import type { RemoteProject, RemoteProjectEntry } from "@/lib/remoteTransport";

/** Where an open project is shown, or `null` for a closed recent one. */
export function projectBadge(entry: RemoteProjectEntry): string | null {
  if (entry.desktopWindow) return "PC 창에서 열림";
  if (entry.open) return "원격으로 열림";
  return null;
}

/**
 * Whether the phone may close the project. One a PC window shows stays open
 * for that window, so the phone only leaves it (by going back).
 */
export function canCloseProject(entry: RemoteProjectEntry): boolean {
  return entry.open && !entry.desktopWindow && entry.projectId !== undefined;
}

/**
 * Whether a close refusal can be overridden by stopping the work, as the PC's
 * "창을 닫을까요?" 닫기 does: refused for running work (`busy`) or a build
 * (`building`), not for a PC Map window (`companions`) or another open in
 * progress (`switching`). The PC names the kind; its wording may change.
 */
export function canForceClose(code: string | undefined): boolean {
  return code === "busy" || code === "building";
}

export interface ProjectActions {
  attach(projectId: string): Promise<RemoteProject>;
  openProject(path: string): Promise<RemoteProject>;
}

/** Work on `entry`: attach to it when it is open, otherwise open it on the PC. */
export function openEntry(actions: ProjectActions, entry: RemoteProjectEntry): Promise<RemoteProject> {
  if (entry.open && entry.projectId !== undefined) return actions.attach(entry.projectId);
  return actions.openProject(entry.path);
}
