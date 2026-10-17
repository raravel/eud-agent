import { describe, expect, it } from "vitest";

import { isSteerMessage } from "@/lib/protocol";
import { steerAsChat, steerDraft, whenTurnEnds } from "@/lib/steer";
import { createPanelStore, type PendingSteer } from "@/state/store";

function runningStore() {
  const store = createPanelStore();
  store.wsOpen();
  store.applyList({ files: [] });
  store.log("you", "first task");
  store.chatSent();
  return store;
}

const pending = (steerId: string, text: string): PendingSteer => ({
  steerId,
  text,
  attachments: [
    { id: `att-${steerId}`, name: "note.txt", mime: "text/plain", kind: "text", size: 4 },
  ],
  mentions: [],
});

describe("mid-turn steering in the session store", () => {
  it("shows a sent message as waiting until the turn receives it", () => {
    const store = runningStore();
    store.steerSent(pending("s1", "make it red"));
    expect(store.getState().pendingSteers.map((steer) => steer.text)).toEqual([
      "make it red",
    ]);
    expect(store.getState().log.map((entry) => entry.text)).toEqual(["first task"]);
  });

  it("puts a received message where the stream reached and keeps the turn running", () => {
    const store = runningStore();
    store.agentEvent("delta", "Working on it.");
    store.steerSent(pending("s1", "make it red"));
    store.steerApplied([{ steerId: "s1", text: "make it red" }]);

    const state = store.getState();
    expect(state.pendingSteers).toEqual([]);
    expect(state.log.map((entry) => [entry.kind, entry.text])).toEqual([
      ["you", "first task"],
      ["agent", "Working on it."],
      ["you", "make it red"],
    ]);
    expect(state.log[2].attachments?.[0]?.name).toBe("note.txt");
    expect(state.phase).toBe("thinking");

    store.agentEvent("delta", "Red it is.");
    // The final answer repeats every message of the turn; only what streamed
    // after the steer is still to be logged.
    store.answerReceived("Working on it. Red it is.");
    expect(store.getState().log.map((entry) => entry.text)).toEqual([
      "first task",
      "Working on it.",
      "make it red",
      "Red it is.",
    ]);
  });

  it("records a message from another client by the text its event carries", () => {
    const store = runningStore();
    store.steerApplied([{ steerId: "phone", text: "from the phone" }]);
    expect(store.getState().log.at(-1)).toMatchObject({
      kind: "you",
      text: "from the phone",
    });
  });

  it("starts the turn the engine began for messages the last turn never received", () => {
    const store = runningStore();
    store.steerSent(pending("s1", "make it red"));
    store.answerReceived("done");
    expect(store.getState().phase).toBe("ready");

    store.steerTurnStarted([{ steerId: "s1", text: "make it red" }], "turn-2");
    const state = store.getState();
    expect(state.pendingSteers).toEqual([]);
    expect(state.phase).toBe("thinking");
    expect(state.log.at(-1)).toMatchObject({
      kind: "you",
      text: "make it red",
      clientTurnId: "turn-2",
    });
  });

  it("hands a waiting message back once", () => {
    const store = runningStore();
    store.steerSent(pending("s1", "make it red"));
    expect(store.steerTaken(["s1"]).map((steer) => steer.text)).toEqual(["make it red"]);
    expect(store.steerTaken(["s1"])).toEqual([]);
    expect(store.getState().pendingSteers).toEqual([]);
  });
});

describe("steer helpers", () => {
  it("waits for the running turn to end", async () => {
    const store = runningStore();
    let ended = false;
    const waiting = whenTurnEnds(store).then(() => {
      ended = true;
    });
    await Promise.resolve();
    expect(ended).toBe(false);
    store.answerReceived("done");
    await waiting;
    expect(ended).toBe(true);
    await whenTurnEnds(store);
  });

  it("joins returned messages into one composer draft in their order", () => {
    const draft = steerDraft([pending("a", "first"), pending("b", ""), pending("c", "third")]);
    expect(draft.text).toBe("first\n\nthird");
    expect(draft.attachments.map((attachment) => attachment.id)).toEqual([
      "att-a",
      "att-b",
      "att-c",
    ]);
    expect(draft.executionMode).toBe("interactive");
    expect(draft.clarify).toBeUndefined();
    expect(draft.skillSelections).toBeUndefined();
  });

  it("sends a message without a running turn as an ordinary one without toggles", () => {
    expect(steerAsChat({ text: "go", attachments: [], mentions: [] })).toEqual({
      text: "go",
      attachments: [],
      mentions: [],
      executionMode: "interactive",
    });
  });

  it("accepts only well-formed steer events", () => {
    const event = {
      type: "steer",
      sessionId: "session-a",
      kind: "applied",
      steers: [{ steerId: "s1", text: "red" }],
    };
    expect(isSteerMessage(event)).toBe(true);
    expect(isSteerMessage({ ...event, kind: "queued" })).toBe(false);
    expect(isSteerMessage({ ...event, steers: [{ steerId: 1 }] })).toBe(false);
    expect(isSteerMessage({ ...event, sessionId: "" })).toBe(false);
  });
});
