import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  CONNECTION_HEADER,
  DETACHED_EVENT,
  DISCONNECTED_MESSAGE,
  RESYNC_EVENT,
  RemoteTransport,
  UNAUTHORIZED_MESSAGE,
  type SocketLike,
} from "./remoteTransport";

class FakeSocket implements SocketLike {
  onopen: SocketLike["onopen"] = null;
  onmessage: SocketLike["onmessage"] = null;
  onclose: SocketLike["onclose"] = null;
  onerror: SocketLike["onerror"] = null;
  sent: unknown[] = [];
  closed = false;

  send(data: string): void {
    this.sent.push(JSON.parse(data));
  }

  close(): void {
    this.closed = true;
  }

  receive(message: unknown): void {
    this.onmessage?.({ data: JSON.stringify(message) });
  }

  drop(): void {
    this.onclose?.({});
  }
}

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function setup(fetchImpl?: (url: string, init?: RequestInit) => Promise<Response>) {
  const sockets: FakeSocket[] = [];
  const fetchFn = vi.fn(fetchImpl ?? (async () => jsonResponse({ ok: true, result: null })));
  const transport = new RemoteTransport({
    fetch: fetchFn,
    socketUrl: "ws://pc.example/remote/api/events",
    createSocket: () => {
      const socket = new FakeSocket();
      sockets.push(socket);
      return socket;
    },
    reconnectDelays: [100, 200],
    connectionTimeoutMs: 1_000,
  });
  return { transport, sockets, fetchFn };
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("RemoteTransport", () => {
  it("names the socket's connection on every command and unwraps the result", async () => {
    const { transport, sockets, fetchFn } = setup(async () =>
      jsonResponse({ ok: true, result: { project: "Sample" } }),
    );
    const pending = transport.invoke("status");
    expect(sockets).toHaveLength(1);
    sockets[0].receive({ type: "hello", connId: "c1" });
    await expect(pending).resolves.toEqual({ project: "Sample" });
    const [url, init] = fetchFn.mock.calls[0];
    expect(url).toBe("/remote/api/invoke/status");
    expect(init?.method).toBe("POST");
    expect((init?.headers as Record<string, string>)[CONNECTION_HEADER]).toBe("c1");
    expect(init?.body).toBe("{}");
    expect(transport.currentStatus()).toBe("open");
  });

  it("rejects with the app's error string, like a desktop invoke", async () => {
    const { transport, sockets } = setup(async (url) =>
      url.endsWith("/forbidden")
        ? jsonResponse(
            { ok: false, code: "not_allowed", error: "원격 접속에서는 사용할 수 없는 기능입니다." },
            403,
          )
        : jsonResponse({ ok: false, error: "실행 중인 세션은 삭제할 수 없습니다." }),
    );
    transport.start();
    sockets[0].receive({ type: "hello", connId: "c1" });
    await expect(transport.invoke("session_delete", { id: "s" })).rejects.toBe(
      "실행 중인 세션은 삭제할 수 없습니다.",
    );
    await expect(transport.invoke("forbidden")).rejects.toBe(
      "원격 접속에서는 사용할 수 없는 기능입니다.",
    );
  });

  it("reports an unpaired device and stops reconnecting", async () => {
    const statuses: string[] = [];
    const { transport, sockets } = setup(async () =>
      jsonResponse({ error: "연결된 기기가 아닙니다." }, 401),
    );
    transport.onStatus((status) => statuses.push(status));
    transport.start();
    sockets[0].receive({ type: "hello", connId: "c1" });
    await expect(transport.invoke("status")).rejects.toBe("연결된 기기가 아닙니다.");
    expect(transport.currentStatus()).toBe("unauthorized");
    sockets[0].drop();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(sockets).toHaveLength(1);
    await expect(transport.invoke("status")).rejects.toBe(UNAUTHORIZED_MESSAGE);
    expect(statuses).toContain("unauthorized");
  });

  it("fails a command with a recovery message when the PC cannot be reached", async () => {
    const { transport, sockets } = setup(async () => {
      throw new TypeError("Failed to fetch");
    });
    transport.start();
    sockets[0].receive({ type: "hello", connId: "c1" });
    await expect(transport.invoke("status")).rejects.toBe(DISCONNECTED_MESSAGE);
  });

  it("waits for a connection, then gives up with a recovery message", async () => {
    const { transport } = setup();
    const pending = transport.invoke("status");
    const assertion = expect(pending).rejects.toBe(DISCONNECTED_MESSAGE);
    await vi.advanceTimersByTimeAsync(1_000);
    await assertion;
  });

  it("returns binary results as an ArrayBuffer", async () => {
    const { transport, sockets } = setup(
      async () =>
        new Response(new Uint8Array([1, 2, 3]), {
          headers: { "content-type": "application/octet-stream" },
        }),
    );
    transport.start();
    sockets[0].receive({ type: "hello", connId: "c1" });
    const result = await transport.invoke<ArrayBuffer>("map_agent_render");
    expect(result).toBeInstanceOf(ArrayBuffer);
    expect([...new Uint8Array(result)]).toEqual([1, 2, 3]);
  });

  it("uploads an attachment as the raw body with the desktop's headers", async () => {
    const { transport, sockets, fetchFn } = setup(async () =>
      jsonResponse({ ok: true, result: { id: "a1" } }),
    );
    transport.start();
    sockets[0].receive({ type: "hello", connId: "c1" });
    const result = await transport.invoke("attachment_stage", new Uint8Array([9, 9]), {
      headers: { "x-eud-file-name-hex": "61", "content-type": "image/png" },
    });
    expect(result).toEqual({ id: "a1" });
    const [url, init] = fetchFn.mock.calls[0];
    expect(url).toBe("/remote/api/attachment_stage");
    const headers = init?.headers as Record<string, string>;
    expect(headers["x-eud-file-name-hex"]).toBe("61");
    expect(headers["content-type"]).toBe("image/png");
    expect(headers[CONNECTION_HEADER]).toBe("c1");
    expect(init?.body).toBeInstanceOf(Blob);
    expect((init?.body as Blob).size).toBe(2);
    // Raw bodies go nowhere else.
    await expect(transport.invoke("chat", new Uint8Array([1]))).rejects.toMatch("사용할 수 없는");
  });

  it("delivers socket events to listeners until they unlisten", async () => {
    const { transport, sockets } = setup();
    transport.start();
    const received: unknown[] = [];
    const unlisten = await transport.listen("answer", (event) => received.push(event.payload));
    sockets[0].receive({ type: "event", event: "answer", payload: { text: "hi" } });
    sockets[0].receive({ type: "event", event: "plan", payload: { text: "other" } });
    unlisten();
    sockets[0].receive({ type: "event", event: "answer", payload: { text: "late" } });
    expect(received).toEqual([{ text: "hi" }]);
  });

  it("reconnects with backoff, re-attaches, then asks for a resync", async () => {
    const { transport, sockets } = setup(async () =>
      jsonResponse({ ok: true, result: { projectId: "p1", name: "Sample", root: "C:/p" } }),
    );
    const resyncs: unknown[] = [];
    await transport.listen(RESYNC_EVENT, (event) => resyncs.push(event.payload));
    transport.start();
    sockets[0].receive({ type: "hello", connId: "c1" });
    await transport.attach("p1");
    expect(transport.attachedProjectId()).toBe("p1");
    expect(resyncs).toHaveLength(0);

    sockets[0].drop();
    expect(transport.currentStatus()).toBe("closed");
    await vi.advanceTimersByTimeAsync(99);
    expect(sockets).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(1);
    expect(sockets).toHaveLength(2);
    // The second attempt fails too: the next delay is longer.
    sockets[1].drop();
    await vi.advanceTimersByTimeAsync(199);
    expect(sockets).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(1);
    expect(sockets).toHaveLength(3);

    sockets[2].receive({ type: "hello", connId: "c2" });
    expect(sockets[2].sent).toEqual([{ type: "attach", projectId: "p1" }]);
    expect(resyncs).toHaveLength(0);
    sockets[2].receive({ type: "attached", project: { projectId: "p1" } });
    expect(resyncs).toHaveLength(1);
    expect(transport.currentStatus()).toBe("open");
  });

  it("asks for a resync when the app dropped events for a slow socket", async () => {
    const { transport, sockets } = setup();
    const resyncs: unknown[] = [];
    await transport.listen(RESYNC_EVENT, () => resyncs.push(true));
    transport.start();
    sockets[0].receive({ type: "hello", connId: "c1" });
    sockets[0].receive({ type: "resync" });
    expect(resyncs).toHaveLength(1);
  });

  it("forgets a project the desktop closed and says why", async () => {
    const { transport, sockets } = setup(async () => jsonResponse({ ok: true, result: {} }));
    const detached: unknown[] = [];
    await transport.listen(DETACHED_EVENT, (event) => detached.push(event.payload));
    transport.start();
    sockets[0].receive({ type: "hello", connId: "c1" });
    await transport.attach("p1");
    sockets[0].receive({ type: "detached", error: "이 프로젝트는 PC에서 열려 있지 않습니다." });
    expect(transport.attachedProjectId()).toBeNull();
    expect(detached).toEqual([{ error: "이 프로젝트는 PC에서 열려 있지 않습니다." }]);
  });

  it("opens a new connection when the app no longer knows the old one", async () => {
    const { transport, sockets } = setup(async () =>
      jsonResponse({ ok: false, code: "no_connection", error: "PC와의 연결을 찾을 수 없습니다." }, 409),
    );
    transport.start();
    sockets[0].receive({ type: "hello", connId: "c1" });
    await expect(transport.invoke("status")).rejects.toBe("PC와의 연결을 찾을 수 없습니다.");
    expect(sockets[0].closed).toBe(true);
    await vi.advanceTimersByTimeAsync(100);
    expect(sockets).toHaveLength(2);
  });

  it("lists open projects without waiting for a connection", async () => {
    const { transport, fetchFn } = setup(async () => jsonResponse({ ok: true, result: [] }));
    await expect(transport.invoke("remote_projects")).resolves.toEqual([]);
    const headers = fetchFn.mock.calls[0][1]?.headers as Record<string, string>;
    expect(headers[CONNECTION_HEADER]).toBeUndefined();
  });
});
