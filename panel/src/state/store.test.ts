import { describe, it, expect } from "vitest";
import {
  createPanelStore,
  validateNewEpsName,
  MAX_LOG_ENTRIES,
  type PanelState,
} from "@/state/store";

function freshStore() {
  return createPanelStore();
}

describe("initial state", () => {
  it("starts in connecting with an empty log and no project", () => {
    const store = freshStore();
    const s = store.getState();
    expect(s.phase).toBe("connecting");
    expect(s.log).toEqual([]);
    expect(s.hasProject).toBe(false);
    expect(s.files).toEqual([]);
  });
});

describe("state machine transitions (per spec mermaid)", () => {
  it("connecting -> ready on wsOpen", () => {
    const store = freshStore();
    store.wsOpen();
    expect(store.getState().phase).toBe("ready");
  });

  it("connecting -> retry on wsError", () => {
    const store = freshStore();
    store.wsError();
    expect(store.getState().phase).toBe("retry");
  });

  it("retry -> connecting on wsConnecting", () => {
    const store = freshStore();
    store.wsError();
    expect(store.getState().phase).toBe("retry");
    store.wsConnecting();
    expect(store.getState().phase).toBe("connecting");
  });

  it("ready -> working on instructSent", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.instructSent();
    expect(store.getState().phase).toBe("working");
  });

  it("working -> reviewing on code event", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.instructSent();
    store.codeReceived({ code: "x", lang: "eps", diff: "", diagnostics: [] });
    expect(store.getState().phase).toBe("reviewing");
  });

  it("working -> ready on error event", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.instructSent();
    store.errorReceived("boom");
    expect(store.getState().phase).toBe("ready");
  });

  it("reviewing -> applying on applySent", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.instructSent();
    store.codeReceived({ code: "x", lang: "eps", diff: "", diagnostics: [] });
    store.applySent();
    expect(store.getState().phase).toBe("applying");
  });

  it("applying -> ready on applied event", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.instructSent();
    store.codeReceived({ code: "x", lang: "eps", diff: "", diagnostics: [] });
    store.applySent();
    store.appliedReceived("a.eps");
    expect(store.getState().phase).toBe("ready");
  });

  it("applying -> waiting on progress waiting_build", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.instructSent();
    store.codeReceived({ code: "x", lang: "eps", diff: "", diagnostics: [] });
    store.applySent();
    store.progressReceived("waiting_build");
    expect(store.getState().phase).toBe("waiting");
  });

  it("waiting -> ready on applied", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.instructSent();
    store.codeReceived({ code: "x", lang: "eps", diff: "", diagnostics: [] });
    store.applySent();
    store.progressReceived("waiting_build");
    store.appliedReceived("a.eps");
    expect(store.getState().phase).toBe("ready");
  });

  it("waiting -> ready on error", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.instructSent();
    store.codeReceived({ code: "x", lang: "eps", diff: "", diagnostics: [] });
    store.applySent();
    store.progressReceived("waiting_build");
    store.errorReceived("nope");
    expect(store.getState().phase).toBe("ready");
  });

  it("reviewing -> working on re-instruct (refine)", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.instructSent();
    store.codeReceived({ code: "x", lang: "eps", diff: "", diagnostics: [] });
    expect(store.getState().phase).toBe("reviewing");
    store.instructSent();
    expect(store.getState().phase).toBe("working");
  });
});

describe("applying-reconnect resets to ready (edge case parity)", () => {
  it("a wsOpen while applying resets the phase to ready", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.instructSent();
    store.codeReceived({ code: "x", lang: "eps", diff: "", diagnostics: [] });
    store.applySent();
    expect(store.getState().phase).toBe("applying");
    // reconnect lands an open before the applied confirmation arrives
    store.wsConnecting();
    store.wsOpen();
    expect(store.getState().phase).toBe("ready");
  });

  it("a wsOpen while waiting also resets to ready", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.instructSent();
    store.codeReceived({ code: "x", lang: "eps", diff: "", diagnostics: [] });
    store.applySent();
    store.progressReceived("waiting_build");
    store.wsOpen();
    expect(store.getState().phase).toBe("ready");
  });
});

describe("event log cap (drop oldest, max 500)", () => {
  it("exposes the cap constant of 500", () => {
    expect(MAX_LOG_ENTRIES).toBe(500);
  });

  it("caps the log at MAX_LOG_ENTRIES, dropping the oldest", () => {
    const store = freshStore();
    for (let i = 0; i < MAX_LOG_ENTRIES + 50; i++) {
      store.log("info", `line ${i}`);
    }
    const log = store.getState().log;
    expect(log.length).toBe(MAX_LOG_ENTRIES);
    // oldest dropped: first surviving entry is line 50.
    expect(log[0].text).toBe("line 50");
    expect(log[log.length - 1].text).toBe(`line ${MAX_LOG_ENTRIES + 49}`);
  });
});

describe("empty-but-open project Send gating (vanilla advisory fix)", () => {
  it("disables SET send for an open-but-empty project", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [] }); // open project, zero files
    const s = store.getState();
    expect(s.hasProject).toBe(true);
    expect(canSendSet(s)).toBe(false);
  });

  it("keeps new-file mode available for an empty project", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [] });
    store.setNewFileMode(true);
    expect(canSendNewEps(store.getState(), "thing.eps")).toBe(true);
  });

  it("enables SET send when a settable target exists", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({
      files: [{ path: "a.eps", ftype: "CUIEps", settable: true }],
    });
    store.selectTarget("a.eps");
    expect(canSendSet(store.getState())).toBe(true);
  });

  it("disables SET send when the only files are non-settable (GUI)", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({
      files: [{ path: "gui.tgui", ftype: "GUI", settable: false }],
    });
    expect(canSendSet(store.getState())).toBe(false);
  });

  it("disables send entirely with no project", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ error: "no project" });
    const s = store.getState();
    expect(s.hasProject).toBe(false);
    expect(canSendSet(s)).toBe(false);
  });

  it("disables send while busy (working / applying / waiting)", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({ files: [{ path: "a.eps", ftype: "CUIEps", settable: true }] });
    store.selectTarget("a.eps");
    store.instructSent(); // working
    expect(canSendSet(store.getState())).toBe(false);
  });
});

describe("no-project signal via error message (server contract: no list{error})", () => {
  it("error{message:'ERROR: no project'} clears the project + gates SET off", () => {
    const store = freshStore();
    store.wsOpen();
    // A project was open with a settable target...
    store.applyList({
      files: [{ path: "a.eps", ftype: "CUIEps", settable: true }],
    });
    store.selectTarget("a.eps");
    expect(canSendSet(store.getState())).toBe(true);
    // ...then the bridge reports no project via an error (no list{error} path).
    store.errorReceived("ERROR: no project");
    const s = store.getState();
    expect(s.hasProject).toBe(false);
    expect(s.files).toEqual([]);
    expect(s.selectedTarget).toBe("");
    expect(canSendSet(s)).toBe(false);
    expect(s.phase).toBe("ready");
  });

  it("matches the 'no project' literal case-insensitively", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({
      files: [{ path: "a.eps", ftype: "CUIEps", settable: true }],
    });
    store.errorReceived("ERROR: No Project loaded");
    expect(store.getState().hasProject).toBe(false);
  });

  it("an unrelated error does NOT clear an open project", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyList({
      files: [{ path: "a.eps", ftype: "CUIEps", settable: true }],
    });
    store.selectTarget("a.eps");
    store.errorReceived("ERROR: duplicate 'a.eps'");
    const s = store.getState();
    expect(s.hasProject).toBe(true);
    expect(s.files.length).toBe(1);
    expect(canSendSet(s)).toBe(true);
  });
});

describe("status compiling flag (documented status event field)", () => {
  it("stores compiling=true from a status event", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyStatus({ compiling: true, project: "MyMap" });
    const s = store.getState();
    expect(s.compiling).toBe(true);
    expect(s.project).toBe("MyMap");
  });

  it("clears compiling when a later status reports false", () => {
    const store = freshStore();
    store.wsOpen();
    store.applyStatus({ compiling: true, project: "MyMap" });
    store.applyStatus({ compiling: false, project: "MyMap" });
    expect(store.getState().compiling).toBe(false);
  });

  it("defaults compiling to false initially", () => {
    expect(freshStore().getState().compiling).toBe(false);
  });
});

// Helpers that read the store's derived selectors. The store must expose
// these so the (future) UI does not re-derive gating logic.
function canSendSet(s: PanelState): boolean {
  return s.canSendSet;
}
function canSendNewEps(s: PanelState, name: string): boolean {
  return s.canSendNewEps && validateNewEpsName(name).ok;
}

describe("NEWEPS filename validation", () => {
  it("rejects an empty / whitespace-only name", () => {
    expect(validateNewEpsName("").ok).toBe(false);
    expect(validateNewEpsName("   ").ok).toBe(false);
  });

  it("rejects path separators", () => {
    expect(validateNewEpsName("a/b.eps").ok).toBe(false);
    expect(validateNewEpsName("a\\b.eps").ok).toBe(false);
  });

  it("accepts a plain filename (trimmed)", () => {
    const r = validateNewEpsName("  thing.eps  ");
    expect(r.ok).toBe(true);
    if (r.ok) expect(r.name).toBe("thing.eps");
  });
});

describe("subscribe / notify", () => {
  it("notifies subscribers on state change and supports unsubscribe", () => {
    const store = freshStore();
    let count = 0;
    const unsub = store.subscribe(() => {
      count += 1;
    });
    store.wsOpen();
    store.log("info", "hi");
    expect(count).toBeGreaterThanOrEqual(2);
    const after = count;
    unsub();
    store.log("info", "bye");
    expect(count).toBe(after);
  });
});
