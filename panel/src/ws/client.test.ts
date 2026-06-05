import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { WsClient, RECONNECT_BACKOFF_MS } from "@/ws/client";
import type { ClientMessage, ServerMessage } from "@/ws/protocol";

// ---- minimal injectable WebSocket mock --------------------------------
// Mirrors the parts of the DOM WebSocket the client uses. Tests drive the
// lifecycle manually (open / message / error / close) instead of a real socket.
const CONNECTING = 0;
const OPEN = 1;
const CLOSING = 2;
const CLOSED = 3;

class MockWebSocket {
  static OPEN = OPEN;
  static CONNECTING = CONNECTING;
  static CLOSING = CLOSING;
  static CLOSED = CLOSED;

  static instances: MockWebSocket[] = [];

  url: string;
  readyState = CONNECTING;
  sent: string[] = [];

  onopen: (() => void) | null = null;
  onmessage: ((ev: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;

  constructor(url: string) {
    this.url = url;
    MockWebSocket.instances.push(this);
  }

  send(data: string) {
    this.sent.push(data);
  }

  close() {
    this.readyState = CLOSED;
  }

  // ---- test drivers ----
  fireOpen() {
    this.readyState = OPEN;
    this.onopen?.();
  }
  fireMessage(obj: unknown) {
    this.onmessage?.({ data: JSON.stringify(obj) });
  }
  fireRaw(data: string) {
    this.onmessage?.({ data });
  }
  fireError() {
    this.onerror?.();
  }
  fireClose() {
    this.readyState = CLOSED;
    this.onclose?.();
  }

  sentMessages(): ClientMessage[] {
    return this.sent.map((s) => JSON.parse(s) as ClientMessage);
  }
}

function makeFactory() {
  return (url: string) => new MockWebSocket(url) as unknown as WebSocket;
}

function lastSocket(): MockWebSocket {
  return MockWebSocket.instances[MockWebSocket.instances.length - 1];
}

beforeEach(() => {
  MockWebSocket.instances = [];
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("token + url derivation", () => {
  it("reads the token from location.search and builds the ws url", () => {
    const received: ServerMessage[] = [];
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=abc123", host: "127.0.0.1:8765" },
      onMessage: (m) => received.push(m),
    });
    client.connect();
    expect(lastSocket().url).toBe(
      "ws://127.0.0.1:8765/ws?token=abc123",
    );
  });

  it("url-encodes the token", () => {
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=a%20b/c", host: "h:1" },
      onMessage: () => {},
    });
    client.connect();
    // The decoded token is "a b/c"; it must be re-encoded in the URL.
    expect(lastSocket().url).toContain("token=a%20b%2Fc");
  });

  it("tolerates a missing token (empty)", () => {
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "", host: "h:1" },
      onMessage: () => {},
    });
    client.connect();
    expect(lastSocket().url).toBe("ws://h:1/ws?token=");
  });
});

describe("on (re)open re-requests status + list", () => {
  it("sends status then list on open", () => {
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=t", host: "h:1" },
      onMessage: () => {},
    });
    client.connect();
    lastSocket().fireOpen();
    const types = lastSocket().sentMessages().map((m) => m.type);
    expect(types).toContain("status");
    expect(types).toContain("list");
  });

  it("re-requests status + list again after a reconnect", () => {
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=t", host: "h:1" },
      onMessage: () => {},
    });
    client.connect();
    const first = lastSocket();
    first.fireOpen();
    first.fireClose(); // outage
    vi.advanceTimersByTime(RECONNECT_BACKOFF_MS);
    const second = lastSocket();
    expect(second).not.toBe(first);
    second.fireOpen();
    const types = second.sentMessages().map((m) => m.type);
    expect(types).toContain("status");
    expect(types).toContain("list");
  });
});

describe("reconnect schedule (2s backoff)", () => {
  it("reconnects after RECONNECT_BACKOFF_MS on close", () => {
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=t", host: "h:1" },
      onMessage: () => {},
    });
    client.connect();
    const first = lastSocket();
    first.fireOpen();
    expect(MockWebSocket.instances.length).toBe(1);
    first.fireClose();
    // not reconnected yet
    vi.advanceTimersByTime(RECONNECT_BACKOFF_MS - 1);
    expect(MockWebSocket.instances.length).toBe(1);
    vi.advanceTimersByTime(1);
    expect(MockWebSocket.instances.length).toBe(2);
  });

  it("does not stack multiple reconnect timers", () => {
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=t", host: "h:1" },
      onMessage: () => {},
    });
    client.connect();
    const first = lastSocket();
    first.fireError();
    first.fireClose();
    vi.advanceTimersByTime(RECONNECT_BACKOFF_MS);
    // exactly one new socket, not two
    expect(MockWebSocket.instances.length).toBe(2);
  });
});

describe("single-disconnect-log fix (vanilla advisory)", () => {
  it("logs ONE disconnect entry for an outage across many retry cycles", () => {
    const logs: string[] = [];
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=t", host: "h:1" },
      onMessage: () => {},
      onLog: (kind, _text) => {
        if (kind === "disconnect") logs.push(_text);
      },
    });
    client.connect();
    const first = lastSocket();
    first.fireOpen(); // was-open = true
    // Outage begins: error then close, then several failed retry cycles.
    first.fireError();
    first.fireClose();
    for (let i = 0; i < 5; i++) {
      vi.advanceTimersByTime(RECONNECT_BACKOFF_MS);
      const sock = lastSocket();
      sock.fireError(); // retry fails (server still down)
      sock.fireClose();
    }
    // Exactly ONE disconnect log for the whole outage.
    expect(logs.length).toBe(1);
  });

  it("logs a fresh disconnect again after a successful reconnect then a new outage", () => {
    const logs: string[] = [];
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=t", host: "h:1" },
      onMessage: () => {},
      onLog: (kind) => {
        if (kind === "disconnect") logs.push("x");
      },
    });
    client.connect();
    const first = lastSocket();
    first.fireOpen();
    first.fireClose(); // outage 1 -> 1 log
    vi.advanceTimersByTime(RECONNECT_BACKOFF_MS);
    const second = lastSocket();
    second.fireOpen(); // recovered
    second.fireClose(); // outage 2 -> 1 more log
    expect(logs.length).toBe(2);
  });
});

describe("inbound dispatch", () => {
  it("dispatches a known server message", () => {
    const received: ServerMessage[] = [];
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=t", host: "h:1" },
      onMessage: (m) => received.push(m),
    });
    client.connect();
    lastSocket().fireOpen();
    lastSocket().fireMessage({ type: "answer", text: "done" });
    expect(received.some((m) => m.type === "answer")).toBe(true);
  });

  it("surfaces unknown message types via onLog and never throws", () => {
    const logs: Array<{ kind: string; text: string }> = [];
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=t", host: "h:1" },
      onMessage: () => {
        throw new Error("onMessage must NOT be called for unknown types");
      },
      onLog: (kind, text) => logs.push({ kind, text }),
    });
    client.connect();
    lastSocket().fireOpen();
    expect(() =>
      lastSocket().fireMessage({ type: "mystery", foo: 1 }),
    ).not.toThrow();
    expect(logs.some((l) => l.kind === "unknown")).toBe(true);
  });

  it("tolerates malformed JSON (logs, no throw)", () => {
    const logs: string[] = [];
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=t", host: "h:1" },
      onMessage: () => {},
      onLog: (kind) => logs.push(kind),
    });
    client.connect();
    lastSocket().fireOpen();
    expect(() => lastSocket().fireRaw("{not json")).not.toThrow();
    expect(logs.length).toBeGreaterThan(0);
  });
});

describe("send", () => {
  it("sends when the socket is open", () => {
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=t", host: "h:1" },
      onMessage: () => {},
    });
    client.connect();
    lastSocket().fireOpen();
    const before = lastSocket().sent.length;
    const ok = client.send({ type: "list" });
    expect(ok).toBe(true);
    expect(lastSocket().sent.length).toBe(before + 1);
  });

  it("returns false (no throw) when the socket is not open", () => {
    const client = new WsClient({
      socketFactory: makeFactory(),
      location: { search: "?token=t", host: "h:1" },
      onMessage: () => {},
    });
    client.connect();
    // never fired open
    const ok = client.send({ type: "list" });
    expect(ok).toBe(false);
  });
});
