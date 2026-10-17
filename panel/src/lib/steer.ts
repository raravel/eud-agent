/**
 * Mid-turn steering helpers shared by the desktop panel and the phone.
 *
 * Enter while a turn runs sends the message to that turn (`chat_steer`); the
 * model receives it at its next tool boundary. When no turn runs any more the
 * command answers `no_turn` and the message is sent as an ordinary turn once
 * the panel has seen the previous turn end.
 */
import type { ChatPayload, SteerPayload } from "@/components/InstructionBox";
import { isBusyPhase, type PanelStore, type PendingSteer } from "@/state/store";

/** Resolves once `store` shows no turn in flight. */
export function whenTurnEnds(store: PanelStore): Promise<void> {
  return new Promise((resolve) => {
    if (!isBusyPhase(store.getState().phase)) {
      resolve();
      return;
    }
    const unsubscribe = store.subscribe((state) => {
      if (isBusyPhase(state.phase)) return;
      unsubscribe();
      resolve();
    });
  });
}

/** Waiting messages as one composer draft, in the order they were sent. */
export function steerDraft(steers: readonly PendingSteer[]): ChatPayload {
  const mentions: ChatPayload["mentions"] = [];
  for (const mention of steers.flatMap((steer) => steer.mentions)) {
    if (!mentions.some((existing) => existing.id === mention.id)) {
      mentions.push({ ...mention });
    }
  }
  return {
    text: steers
      .map((steer) => steer.text)
      .filter((text) => text.length > 0)
      .join("\n\n"),
    attachments: steers.flatMap((steer) => steer.attachments),
    mentions,
    executionMode: "interactive",
  };
}

/** A steer that found no running turn, as an ordinary message. */
export function steerAsChat(steer: SteerPayload): ChatPayload {
  return {
    text: steer.text,
    attachments: [...steer.attachments],
    mentions: steer.mentions.map((mention) => ({ ...mention })),
    executionMode: "interactive",
  };
}
