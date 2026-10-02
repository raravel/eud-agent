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

export type E2SourceKind = "e2s" | "e2p";
export type E2Feature =
  | "projectMap"
  | "dat"
  | "fireGraft"
  | "bin"
  | "customTiles"
  | "buttonsBgm"
  | "grp"
  | "structGui"
  | "plugin"
  | "fileManager"
  | "scdb"
  | "soundBgmReferences"
  | "disabledPayload";
export type E2Phase =
  | "preflight"
  | "resolving"
  | "compiling"
  | "collectingResources"
  | "materializing"
  | "validating"
  | "publishing"
  | "opening"
  | "completed";
export type E2IssueCode =
  | "unsupported_version"
  | "malformed_input"
  | "source_changed"
  | "missing_input_map"
  | "ambiguous_input_map"
  | "active_scdb_unsupported"
  | "active_grp_unsupported"
  | "active_chatwrite_unverified"
  | "active_definition_unknown"
  | "invalid_resource"
  | "unsafe_plugin";
export type E2RelinkKind = "inputMap" | "resource";

export interface E2Issue {
  readonly code: E2IssueCode;
  readonly severity: "info" | "warning" | "fatal";
  readonly message: string;
  readonly path?: string;
  readonly feature?: E2Feature;
  readonly needId?: string;
}

export interface E2RelinkNeed {
  readonly id: string;
  readonly kind: E2RelinkKind;
  readonly label: string;
  readonly originalPath?: string;
  readonly candidates: readonly string[];
  readonly extensions: readonly string[];
}

export interface E2RelinkChoice {
  readonly needId: string;
  readonly path: string;
}

export interface E2PreflightRequest {
  readonly sourcePath: string;
  readonly destinationPath: string;
  readonly relinks: readonly E2RelinkChoice[];
}

export interface E2ImportRequest {
  readonly sourcePath: string;
  readonly destinationPath: string;
  readonly fingerprint: string;
  readonly relinks: readonly E2RelinkChoice[];
}

export interface E2CancelRequest {
  readonly importId: string;
}

export interface E2RelinkPickRequest {
  readonly need: E2RelinkNeed;
}

export interface E2PreflightReady {
  readonly state: "ready";
  readonly fingerprint: string;
  readonly sourceKind: E2SourceKind;
  readonly features: readonly E2Feature[];
  readonly warnings: readonly E2Issue[];
}

export interface E2PreflightNeedsRelink {
  readonly state: "needsRelink";
  readonly fingerprint: string;
  readonly needs: readonly E2RelinkNeed[];
}

export interface E2PreflightFatal {
  readonly state: "fatal";
  readonly fingerprint?: string;
  readonly issues: readonly E2Issue[];
}

export type E2PreflightResult =
  | E2PreflightReady
  | E2PreflightNeedsRelink
  | E2PreflightFatal;

export interface E2ImportProgress {
  readonly state: "progress";
  readonly importId: string;
  readonly fingerprint: string;
  readonly phase: E2Phase;
  readonly completed: number;
  readonly total: number;
  readonly message: string;
}

export interface E2ImportCompleted {
  readonly state: "completed";
  readonly importId: string;
  readonly fingerprint: string;
  readonly projectOpened: true;
  readonly projectPath: string;
}

export interface E2ImportCancelled {
  readonly state: "cancelled";
  readonly importId: string;
  readonly fingerprint: string;
}

export interface E2ImportFatal {
  readonly state: "fatal";
  readonly importId?: string;
  readonly fingerprint?: string;
  readonly issues: readonly E2Issue[];
}

export type E2ImportResult =
  | E2ImportProgress
  | E2ImportCompleted
  | E2ImportCancelled
  | E2ImportFatal;

export interface E2ImportIdentity {
  readonly importId: string;
  readonly fingerprint: string;
}

export function isE2ImportResultFor(
  result: E2ImportResult,
  identity: E2ImportIdentity,
): boolean {
  return result.importId === identity.importId && result.fingerprint === identity.fingerprint;
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

const E2_FEATURES: readonly E2Feature[] = [
  "projectMap",
  "dat",
  "fireGraft",
  "bin",
  "customTiles",
  "buttonsBgm",
  "grp",
  "structGui",
  "plugin",
  "fileManager",
  "scdb",
  "soundBgmReferences",
  "disabledPayload",
];
const E2_PHASES: readonly E2Phase[] = [
  "preflight",
  "resolving",
  "compiling",
  "collectingResources",
  "materializing",
  "validating",
  "publishing",
  "opening",
  "completed",
];
const E2_ISSUE_CODES: readonly E2IssueCode[] = [
  "unsupported_version",
  "malformed_input",
  "source_changed",
  "missing_input_map",
  "ambiguous_input_map",
  "active_scdb_unsupported",
  "active_grp_unsupported",
  "active_chatwrite_unverified",
  "active_definition_unknown",
  "invalid_resource",
  "unsafe_plugin",
];
const E2_RELINK_KINDS: readonly E2RelinkKind[] = ["inputMap", "resource"];

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.trim() !== "";
}

function isOneOf<T extends string>(values: readonly T[], value: unknown): value is T {
  return typeof value === "string" && values.some((candidate) => candidate === value);
}

function optionalNonEmptyString(value: unknown, command: string): string | undefined {
  if (value === undefined) return undefined;
  if (!isNonEmptyString(value)) throw new ProjectImportProtocolError(command);
  return value;
}

function requiredInteger(value: unknown, command: string): number {
  if (typeof value !== "number" || !Number.isInteger(value) || value < 0) {
    throw new ProjectImportProtocolError(command);
  }
  return value;
}

function optionalFeature(value: unknown, command: string): E2Feature | undefined {
  if (value === undefined) return undefined;
  return parseE2Feature(value, command);
}

function parseE2Feature(value: unknown, command: string): E2Feature {
  if (!isOneOf(E2_FEATURES, value)) throw new ProjectImportProtocolError(command);
  return value;
}

function parseE2Issue(value: unknown, command: string): E2Issue {
  if (
    !isObject(value) ||
    !isOneOf(E2_ISSUE_CODES, value.code) ||
    !isOneOf(["info", "warning", "fatal"], value.severity) ||
    !isNonEmptyString(value.message) ||
    (value.needId !== undefined && !isNonEmptyString(value.needId))
  ) {
    throw new ProjectImportProtocolError(command);
  }
  const path = optionalNonEmptyString(value.path, command);
  const needId = optionalNonEmptyString(value.needId, command);
  const feature = optionalFeature(value.feature, command);
  return {
    code: value.code,
    severity: value.severity,
    message: value.message,
    ...(path === undefined ? {} : { path }),
    ...(feature === undefined ? {} : { feature }),
    ...(needId === undefined ? {} : { needId }),
  };
}

function parseE2RelinkNeed(value: unknown, command: string): E2RelinkNeed {
  if (
    !isObject(value) ||
    !isNonEmptyString(value.id) ||
    !isOneOf(E2_RELINK_KINDS, value.kind) ||
    !isNonEmptyString(value.label) ||
    !Array.isArray(value.candidates) ||
    !value.candidates.every(isNonEmptyString) ||
    !Array.isArray(value.extensions) ||
    !value.extensions.every(isNonEmptyString)
  ) {
    throw new ProjectImportProtocolError(command);
  }
  const originalPath = optionalNonEmptyString(value.originalPath, command);
  return {
    id: value.id,
    kind: value.kind,
    label: value.label,
    candidates: value.candidates,
    extensions: value.extensions,
    ...(originalPath === undefined ? {} : { originalPath }),
  };
}

export function decodeE2PreflightResult(value: unknown): E2PreflightResult {
  const command = "setup_e2_preflight";
  if (!isObject(value) || !isNonEmptyString(value.state)) {
    throw new ProjectImportProtocolError(command);
  }
  switch (value.state) {
    case "ready":
      if (
        !isNonEmptyString(value.fingerprint) ||
        !isOneOf(["e2s", "e2p"], value.sourceKind) ||
        !Array.isArray(value.features) ||
        !Array.isArray(value.warnings)
      ) {
        throw new ProjectImportProtocolError(command);
      }
      return {
        state: "ready",
        fingerprint: value.fingerprint,
        sourceKind: value.sourceKind,
        features: value.features.map((feature) => parseE2Feature(feature, command)),
        warnings: value.warnings.map((warning) => parseE2Issue(warning, command)),
      };
    case "needsRelink":
      if (
        !isNonEmptyString(value.fingerprint) ||
        !Array.isArray(value.needs) ||
        value.needs.length === 0
      ) {
        throw new ProjectImportProtocolError(command);
      }
      return {
        state: "needsRelink",
        fingerprint: value.fingerprint,
        needs: value.needs.map((need) => parseE2RelinkNeed(need, command)),
      };
    case "fatal":
      if (!Array.isArray(value.issues) || value.issues.length === 0) {
        throw new ProjectImportProtocolError(command);
      }
      const preflightFingerprint = optionalNonEmptyString(value.fingerprint, command);
      return {
        state: "fatal",
        ...(preflightFingerprint === undefined ? {} : { fingerprint: preflightFingerprint }),
        issues: value.issues.map((issue) => parseE2Issue(issue, command)),
      };
    default:
      throw new ProjectImportProtocolError(command);
  }
}

export function decodeE2ImportResult(
  value: unknown,
  command = "setup_import_e2",
): E2ImportResult {
  if (!isObject(value) || !isNonEmptyString(value.state)) {
    throw new ProjectImportProtocolError(command);
  }
  switch (value.state) {
    case "progress":
      const completed = requiredInteger(value.completed, command);
      const total = requiredInteger(value.total, command);
      if (
        !isNonEmptyString(value.importId) ||
        !isNonEmptyString(value.fingerprint) ||
        !isOneOf(E2_PHASES, value.phase) ||
        completed > total ||
        !isNonEmptyString(value.message)
      ) {
        throw new ProjectImportProtocolError(command);
      }
      return {
        state: "progress",
        importId: value.importId,
        fingerprint: value.fingerprint,
        phase: value.phase,
        completed,
        total,
        message: value.message,
      };
    case "completed":
      if (
        !isNonEmptyString(value.importId) ||
        !isNonEmptyString(value.fingerprint) ||
        value.projectOpened !== true ||
        !isNonEmptyString(value.projectPath)
      ) {
        throw new ProjectImportProtocolError(command);
      }
      return {
        state: "completed",
        importId: value.importId,
        fingerprint: value.fingerprint,
        projectOpened: true,
        projectPath: value.projectPath,
      };
    case "cancelled":
      if (!isNonEmptyString(value.importId) || !isNonEmptyString(value.fingerprint)) {
        throw new ProjectImportProtocolError(command);
      }
      return {
        state: "cancelled",
        importId: value.importId,
        fingerprint: value.fingerprint,
      };
    case "fatal":
      if (!Array.isArray(value.issues) || value.issues.length === 0) {
        throw new ProjectImportProtocolError(command);
      }
      const importId = optionalNonEmptyString(value.importId, command);
      const importFingerprint = optionalNonEmptyString(value.fingerprint, command);
      return {
        state: "fatal",
        ...(importId === undefined ? {} : { importId }),
        ...(importFingerprint === undefined ? {} : { fingerprint: importFingerprint }),
        issues: value.issues.map((issue) => parseE2Issue(issue, command)),
      };
    default:
      throw new ProjectImportProtocolError(command);
  }
}

function parseE2Picker(value: unknown, command: string): string | null {
  if (value === null) return null;
  if (!isNonEmptyString(value)) throw new ProjectImportProtocolError(command);
  return value;
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

export async function pickE2Source(
  invoke: InvokeFn = tauriInvoke,
): Promise<string | null> {
  return parseE2Picker(await invoke("setup_pick_e2_source"), "setup_pick_e2_source");
}

export async function pickE2Relink(
  request: E2RelinkPickRequest | E2RelinkNeed,
  invoke: InvokeFn = tauriInvoke,
): Promise<string | null> {
  const need = "need" in request ? request.need : request;
  return parseE2Picker(
    await invoke("setup_pick_e2_relink", { request: { need } }),
    "setup_pick_e2_relink",
  );
}

export async function preflightE2Project(
  request: E2PreflightRequest,
  invoke: InvokeFn = tauriInvoke,
): Promise<E2PreflightResult> {
  return decodeE2PreflightResult(await invoke("setup_e2_preflight", { request }));
}

export async function importE2Project(
  request: E2ImportRequest,
  invoke: InvokeFn = tauriInvoke,
): Promise<E2ImportResult> {
  return decodeE2ImportResult(await invoke("setup_import_e2", { request }));
}

export async function cancelE2Import(
  request: E2CancelRequest | string,
  invoke: InvokeFn = tauriInvoke,
): Promise<E2ImportResult> {
  const cancelRequest = typeof request === "string" ? { importId: request } : request;
  return decodeE2ImportResult(
    await invoke("setup_cancel_e2_import", { request: cancelRequest }),
    "setup_cancel_e2_import",
  );
}
