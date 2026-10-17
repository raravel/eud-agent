import { describe, expect, it } from "vitest";

import { isSteerMessage } from "@/lib/protocol";
import { steerAsChat, steerDraft, steerReturnsToComposer, whenTurnEnds } from "@/lib/steer";
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

describe("mid-turn steering around running tools and edits", () => {
  it("waits for a running tool to finish before placing a received message", () => {
    const store = runningStore();
    store.agentEvent("tool_call", "fs_read", { callId: "c1" });
    store.steerSent(pending("s1", "make it red"));
    store.steerApplied([{ steerId: "s1", text: "make it red" }]);
    // The tool still runs: its card stays live and the message waits.
    expect(store.getState().log.map((entry) => entry.text)).toEqual(["first task"]);
    expect(store.getState().turn.tools[0]?.state).toBe("running");

    store.agentEvent("tool_result", "fs_read", { callId: "c1", status: "completed" });
    const log = store.getState().log;
    expect(log.map((entry) => entry.kind)).toEqual(["you", "info", "you"]);
    expect(log[1].tools?.[0]?.state).toBe("done");
    expect(log[2].text).toBe("make it red");
    expect(store.getState().pendingSteers).toEqual([]);
  });

  it("places a received message when the turn ends even if a tool never reported back", () => {
    const store = runningStore();
    store.agentEvent("tool_call", "fs_read", { callId: "c1" });
    store.steerApplied([{ steerId: "s1", text: "make it red" }]);
    store.answerReceived("done");
    expect(store.getState().log.some((entry) => entry.text === "make it red")).toBe(true);
  });

  it("remembers that the last turn was cancelled until the next one starts", () => {
    const store = runningStore();
    store.cancelSent();
    expect(store.getState().lastTurnCancelled).toBe(true);
    store.chatSent();
    expect(store.getState().lastTurnCancelled).toBe(false);
  });

  it("rewinds to a received message with the part of the turn before it", () => {
    const store = runningStore();
    store.agentEvent("delta", "Working on it.");
    store.steerApplied([{ steerId: "s1", text: "make it red" }]);
    store.answerReceived("Working on it. Done.");
    const steered = store.getState().log.find((entry) => entry.text === "make it red");
    expect(steered?.clientTurnId).toBeUndefined();
    const restored = store.rewindTo(steered!.id);
    expect(restored?.text).toBe("make it red");
    expect(store.getState().log.map((entry) => entry.text)).toEqual([
      "first task",
      "Working on it.",
    ]);
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

  it("returns a message that found no turn to the composer after a cancel or a plan", () => {
    const cancelled = runningStore();
    cancelled.cancelSent();
    expect(steerReturnsToComposer(cancelled.getState())).toBe(true);
    const planned = runningStore();
    planned.planReceived("# plan", 1);
    expect(steerReturnsToComposer(planned.getState())).toBe(true);
    const answered = runningStore();
    answered.answerReceived("done");
    expect(steerReturnsToComposer(answered.getState())).toBe(false);
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
