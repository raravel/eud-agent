import i18n from "@/lib/i18n";
import type { MapConversationEntry } from "./MapAgentPanel";
import type { MapRunPrompt, MapRunTranscript } from "./mapProtocol";

/** Whether the conversation already shows the run's request bubble. */
export function runAlreadyAdopted(
  conversation: MapConversationEntry[],
  requestId: string,
): boolean {
  return conversation.some((entry) => entry.requestId === requestId);
}

/** The request bubble text for a run: what the user typed, or the EPS session's goal. */
export function runPromptText(prompt: MapRunPrompt): string {
  const text = prompt.text.trim();
  const body =
    text ||
    (prompt.mentions.length > 0
      ? i18n.t("map.runAdoption.mentions")
      : i18n.t("map.runAdoption.attachments"));
  if (prompt.origin !== "team") return body;
  const parent = prompt.parentSessionName?.trim() || "EPS";
  return i18n.t("map.runAdoption.teamRequest", { parent, body });
}

/**
 * The entries that put an adopted run into the conversation before its
 * events replay: an omission notice when the transcript was truncated, then
 * the "you" bubble keyed by the request id.
 */
export function runAdoptionEntries(
  run: MapRunTranscript,
  logSequence: number,
): { entries: MapConversationEntry[]; logSequence: number } {
  const entries: MapConversationEntry[] = [];
  let sequence = logSequence;
  if (run.truncated) {
    sequence += 1;
    entries.push({
      id: sequence,
      kind: "info",
      text: i18n.t("map.runAdoption.truncated"),
    });
  }
  sequence += 1;
  entries.push({
    id: sequence,
    kind: "you",
    requestId: run.requestId,
    text: runPromptText(run.prompt),
    mapMentions: run.prompt.mentions,
  });
  return { entries, logSequence: sequence };
}

/** Whether the replayed events already end the turn (an answer or an error). */
export function runHasTerminalEvent(run: MapRunTranscript): boolean {
  return run.events.some(
    (event) => event.name === "answer" || event.name === "error",
  );
}

/**
 * Give the request bubble this window sent (it did not know the request id
 * yet) the id the backend announced, so a later bootstrap recognizes it.
 */
export function stampRunRequestId(
  conversation: MapConversationEntry[],
  requestId: string,
): MapConversationEntry[] {
  if (runAlreadyAdopted(conversation, requestId)) return conversation;
  for (let index = conversation.length - 1; index >= 0; index -= 1) {
    const entry = conversation[index];
    if (entry.kind !== "you") continue;
    if (entry.requestId !== undefined) return conversation;
    const stamped = conversation.slice();
    stamped[index] = { ...entry, requestId };
    return stamped;
  }
  return conversation;
}
