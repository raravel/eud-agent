import { invoke as tauriInvoke } from "@tauri-apps/api/core";

import type { InvokeFn } from "@/lib/ipc";
import { isServerMessage, type SetupMessage } from "@/lib/protocol";

export interface E3sSourceSelection {
  readonly path: string;
}

export interface E3sDestinationSelection {
  readonly path: string;
  readonly empty: boolean;
}

export interface E3sImportRequest {
  readonly sourceE3s: string;
  readonly destination: string;
  readonly excludedImportItems?: readonly string[];
}


/**
 * Turn backend import diagnostics into a concise Korean summary while keeping
 * the original reason available to callers for an exact technical explanation.
 */
export function formatImportIssueReason(reason: string): string {
  const normalized = reason.trim().toLowerCase();
  if (normalized.includes("approved plan body is unavailable")) {
    return "승인된 계획 본문을 찾을 수 없어 해당 계획을 제외했습니다.";
  }
  if (normalized.includes("invalid utf") || normalized.includes("utf-8") || normalized.includes("utf8")) {
    return "문서 인코딩이 올바르지 않아 해당 항목을 제외했습니다.";
  }
  if (
    normalized.includes("destination already") ||
    normalized.includes("local document is authoritative") ||
    normalized.includes("local plan or approval is authoritative")
  ) {
    return "대상 프로젝트에 이미 같은 자료가 있어 원본을 덮어쓰지 않고 제외했습니다.";
  }
  if (
    normalized.includes("ambiguous") ||
    normalized.includes("cannot be uniquely") ||
    normalized.includes("ownership")
  ) {
    return "이 자료가 현재 프로젝트에 속하는지 확인할 수 없어 안전을 위해 제외했습니다.";
  }
  if (normalized.includes("corrupt") || normalized.includes("unsafe")) {
    return "자료가 손상되었거나 안전하지 않아 제외했습니다.";
  }
  if (normalized.includes("unavailable") || normalized.includes("not found")) {
    return "자료를 읽을 수 없어 해당 항목을 제외했습니다.";
  }
  if (normalized.includes("collision")) {
    return "같은 경로의 자료가 충돌하여 원본을 덮어쓰지 않고 제외했습니다.";
  }
  return /[\u3131-\uD79D]/u.test(reason)
    ? reason.trim()
    : "자료를 안전하게 가져오지 못해 해당 항목을 제외했습니다.";
}
export class ProjectImportProtocolError extends Error {
  readonly name = "ProjectImportProtocolError";
  readonly command: string;

  constructor(command: string) {
    super(`invalid response from ${command}`);
    this.command = command;
  }
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function parseSourceSelection(value: unknown): E3sSourceSelection | null {
  if (value === null) return null;
  if (!isObject(value) || typeof value.path !== "string" || value.path.trim() === "") {
    throw new ProjectImportProtocolError("setup_pick_e3s_source");
  }
  return { path: value.path };
}

function parseDestinationSelection(value: unknown): E3sDestinationSelection | null {
  if (value === null) return null;
  if (
    !isObject(value) ||
    typeof value.path !== "string" ||
    value.path.trim() === "" ||
    typeof value.empty !== "boolean"
  ) {
    throw new ProjectImportProtocolError("setup_pick_import_destination");
  }
  return { path: value.path, empty: value.empty };
}

function parseSetup(value: unknown): SetupMessage {
  if (isObject(value)) {
    const candidate = { ...value, type: "setup" };
    if (isServerMessage(candidate) && candidate.type === "setup") return candidate;
  }
  throw new ProjectImportProtocolError("setup_import_e3s");
}

export async function pickE3sSource(
  invoke: InvokeFn = tauriInvoke,
): Promise<E3sSourceSelection | null> {
  return parseSourceSelection(await invoke("setup_pick_e3s_source"));
}

export async function pickE3sImportDestination(
  invoke: InvokeFn = tauriInvoke,
): Promise<E3sDestinationSelection | null> {
  return parseDestinationSelection(await invoke("setup_pick_import_destination"));
}

export async function importE3sProject(
  request: E3sImportRequest,
  invoke: InvokeFn = tauriInvoke,
): Promise<SetupMessage> {
  return parseSetup(await invoke("setup_import_e3s", { request }));
}
