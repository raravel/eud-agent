import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { RemoteDevice, RemotePairingNotice, RemoteStatus } from "@/lib/ipc";
import { PAIRING_REFRESH_MARGIN_MS, PhoneConnectDialog } from "./PhoneConnectDialog";
import type { RemoteSettingsActions } from "./RemoteSettingsPanel";

const URL_BASE = "https://raravel.github.io/eud-agent/connect/#host=pc.tail1234.ts.net";

function status(
  overrides: Partial<RemoteStatus> = {},
  tailscale: Partial<RemoteStatus["tailscale"]> = {},
): RemoteStatus {
  return {
    enabled: true,
    port: 47821,
    listening: true,
    error: null,
    ...overrides,
    tailscale: {
      installed: true,
      running: true,
      dnsName: "pc.tail1234.ts.net",
      serveConfigured: true,
      funnel: false,
      serveCommand: "tailscale serve --bg --https=443 http://127.0.0.1:47821",
      backendState: "Running",
      installSupported: true,
      loginName: "me@example.com",
      ...tailscale,
    },
  };
}

function pairing(code = "ABCD2345", lifeMs = 5 * 60 * 1000) {
  return { code, expiresAt: Date.now() + lifeMs, url: `${URL_BASE}&pair=${code}` };
}

type Mocked = { [K in keyof RemoteSettingsActions]-?: ReturnType<typeof vi.fn> };

function actions(initial: RemoteStatus, devices: RemoteDevice[] = []): Mocked {
  return {
    status: vi.fn().mockResolvedValue(initial),
    save: vi.fn(async (enabled: boolean, port: number) =>
      status({ enabled, port, listening: enabled }),
    ),
    pairBegin: vi.fn().mockImplementation(async () => pairing()),
    devices: vi.fn().mockResolvedValue(devices),
    revoke: vi.fn(),
    serve: vi.fn(),
    install: vi.fn(),
    login: vi.fn(),
    openUrl: vi.fn().mockResolvedValue(undefined),
    copy: vi.fn(),
    onPairing: vi.fn(() => () => undefined),
    onInstallProgress: vi.fn(() => () => undefined),
  };
}

function renderDialog(calls: Mocked, onOpenChange = vi.fn()) {
  return render(
    <PhoneConnectDialog
      open
      onOpenChange={onOpenChange}
      actions={calls as unknown as RemoteSettingsActions}
      pollMs={10}
    />,
  );
}

const QR_ALT = "휴대폰 카메라로 찍으면 연결 안내 페이지가 열리는 QR 코드";

describe("PhoneConnectDialog", () => {
  it("goes straight to the QR code when remote access is ready", async () => {
    const calls = actions(status());
    renderDialog(calls);

    expect(await screen.findByText("ABCD-2345")).toBeInTheDocument();
    expect(await screen.findByAltText(QR_ALT)).toBeInTheDocument();
    expect(screen.getByText(/휴대폰 카메라로 찍고 'PC 열기'를 누르세요/)).toBeInTheDocument();
    expect(calls.pairBegin).toHaveBeenCalledTimes(1);
    expect(screen.getByText("pc.tail1234.ts.net")).toBeInTheDocument();
    // The countdown runs to the rotation, a margin before the code expires.
    const margin = PAIRING_REFRESH_MARGIN_MS / 1000;
    expect(screen.getByRole("timer")).toHaveTextContent(
      new RegExp(`^${Math.floor((300 - margin) / 60)}:${String((300 - margin) % 60).padStart(2, "0")} `),
    );
    expect(calls.install).not.toHaveBeenCalled();
    expect(calls.save).not.toHaveBeenCalled();
    expect(screen.queryByText("Funnel이 이 포트를 공개 인터넷에 열어 두었습니다")).toBeNull();
  });

  it("warns about Funnel on the QR view", async () => {
    renderDialog(actions(status({}, { funnel: true })));
    expect(
      await screen.findByText("Funnel이 이 포트를 공개 인터넷에 열어 두었습니다"),
    ).toBeInTheDocument();
  });

  it("runs the whole chain: install, sign-in page, server, HTTPS page, QR code", async () => {
    const user = userEvent.setup();
    const missing = status(
      { enabled: false, listening: false },
      { installed: false, running: false, dnsName: null, serveConfigured: false, backendState: null },
    );
    const needsLogin = status(
      { enabled: false, listening: false },
      { running: false, dnsName: null, serveConfigured: false, backendState: "NeedsLogin" },
    );
    const signedIn = status({ enabled: false, listening: false }, { serveConfigured: false });
    const listening = status({}, { serveConfigured: false });
    const calls = actions(missing);
    calls.install.mockResolvedValue({ rebootRequired: false, status: needsLogin });
    const loginUrl = "https://login.tailscale.com/a/abc";
    calls.login.mockResolvedValue({ url: loginUrl, status: needsLogin });
    calls.save.mockResolvedValue(listening);
    const serveUrl = "https://login.tailscale.com/f/serve?node=n1";
    calls.serve.mockResolvedValue({ url: serveUrl, status: listening });
    // Polls while signing in see Tailscale up; polls while serving see it published.
    calls.status
      .mockResolvedValueOnce(missing)
      .mockResolvedValueOnce(missing)
      .mockResolvedValueOnce(needsLogin)
      .mockResolvedValueOnce(signedIn)
      .mockResolvedValueOnce(listening)
      .mockResolvedValue(status());
    renderDialog(calls);

    await user.click(await screen.findByRole("button", { name: "원격 접속 켜기" }));
    expect(await screen.findByText("ABCD-2345")).toBeInTheDocument();

    expect(calls.install).toHaveBeenCalledTimes(1);
    expect(calls.login).toHaveBeenCalledTimes(1);
    expect(calls.save).toHaveBeenCalledWith(true, 47821);
    expect(calls.serve).toHaveBeenCalledTimes(1);
    expect(calls.openUrl.mock.calls).toEqual([[loginUrl], [serveUrl]]);
    expect(calls.pairBegin).toHaveBeenCalledTimes(1);
    expect(calls.install.mock.invocationCallOrder[0]).toBeLessThan(
      calls.login.mock.invocationCallOrder[0],
    );
    expect(calls.login.mock.invocationCallOrder[0]).toBeLessThan(
      calls.save.mock.invocationCallOrder[0],
    );
    expect(calls.save.mock.invocationCallOrder[0]).toBeLessThan(
      calls.serve.mock.invocationCallOrder[0],
    );
  });

  it("shows the current step while it waits for the browser", async () => {
    const user = userEvent.setup();
    const needsLogin = status({}, { running: false, backendState: "NeedsLogin" });
    const calls = actions(needsLogin);
    const loginUrl = "https://login.tailscale.com/a/abc";
    calls.login.mockResolvedValue({ url: loginUrl, status: needsLogin });
    renderDialog(calls);

    await user.click(await screen.findByRole("button", { name: "원격 접속 켜기" }));
    expect(
      await screen.findByText("2/4 브라우저에서 Tailscale 로그인을 마쳐 주세요"),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "브라우저 페이지 다시 열기" }));
    expect(calls.openUrl).toHaveBeenLastCalledWith(loginUrl);

    calls.status.mockResolvedValue(status());
    expect(await screen.findByText("ABCD-2345")).toBeInTheDocument();
  });

  it("stops on an error and resumes from the current state with 다시 시도", async () => {
    const user = userEvent.setup();
    const off = status({ enabled: false, listening: false });
    const calls = actions(off);
    calls.save.mockRejectedValueOnce("포트 47821을(를) 열 수 없습니다(사용 중).");
    renderDialog(calls);

    await user.click(await screen.findByRole("button", { name: "원격 접속 켜기" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "포트 47821을(를) 열 수 없습니다(사용 중).",
    );
    expect(calls.pairBegin).not.toHaveBeenCalled();

    calls.save.mockResolvedValue(status());
    await user.click(screen.getByRole("button", { name: "다시 시도" }));
    expect(await screen.findByText("ABCD-2345")).toBeInTheDocument();
    expect(calls.save).toHaveBeenCalledTimes(2);
    expect(calls.install).not.toHaveBeenCalled();
    expect(calls.login).not.toHaveBeenCalled();
  });

  it("points to the download page where the app cannot install Tailscale", async () => {
    const user = userEvent.setup();
    const calls = actions(
      status({}, { installed: false, running: false, installSupported: false }),
    );
    renderDialog(calls);

    await user.click(await screen.findByRole("button", { name: "원격 접속 켜기" }));
    await user.click(
      await screen.findByRole("button", { name: "Tailscale 내려받기 페이지 열기" }),
    );
    expect(calls.openUrl).toHaveBeenCalledWith("https://tailscale.com/download");
    expect(calls.install).not.toHaveBeenCalled();
  });

  it("asks for a new code shortly before the shown one expires", async () => {
    const calls = actions(status());
    calls.pairBegin
      .mockResolvedValueOnce(pairing("ABCD2345", PAIRING_REFRESH_MARGIN_MS + 50))
      .mockResolvedValue(pairing("WXYZ6789"));
    renderDialog(calls);

    expect(await screen.findByText("ABCD-2345")).toBeInTheDocument();
    expect(await screen.findByText("WXYZ-6789", undefined, { timeout: 2000 })).toBeInTheDocument();
    expect(calls.pairBegin).toHaveBeenCalledTimes(2);
  });

  it("shows the paired phone and offers a new code for another", async () => {
    const user = userEvent.setup();
    let announce: ((notice: RemotePairingNotice) => void) | undefined;
    const calls = actions(status());
    calls.onPairing.mockImplementation((listener: (notice: RemotePairingNotice) => void) => {
      announce = listener;
      return () => {
        announce = undefined;
      };
    });
    renderDialog(calls);

    expect(await screen.findByText("ABCD-2345")).toBeInTheDocument();
    act(() => announce?.({ kind: "paired", deviceId: "d1", name: "Android 휴대폰" }));
    expect(await screen.findByText("연결됨: Android 휴대폰")).toBeInTheDocument();
    expect(screen.queryByText("ABCD-2345")).toBeNull();

    await user.click(screen.getByRole("button", { name: "다른 휴대폰 연결" }));
    expect(await screen.findByText("ABCD-2345")).toBeInTheDocument();
    expect(calls.pairBegin).toHaveBeenCalledTimes(2);
  });

  it("notices a new device in the list without the pairing event", async () => {
    const calls = actions(status());
    calls.onPairing.mockImplementation(() => () => undefined);
    renderDialog(calls);

    expect(await screen.findByText("ABCD-2345")).toBeInTheDocument();
    calls.devices.mockResolvedValue([
      { id: "d9", name: "iPhone", createdAt: 1, lastSeenAt: 1, ownerLogin: null, expired: false },
    ]);
    expect(await screen.findByText("연결됨: iPhone")).toBeInTheDocument();
  });

  it("stops polling once the dialog closes", async () => {
    const user = userEvent.setup();
    const needsLogin = status({}, { running: false, backendState: "NeedsLogin" });
    const calls = actions(needsLogin);
    calls.login.mockResolvedValue({ url: "https://login.tailscale.com/a/abc", status: needsLogin });
    const view = renderDialog(calls);

    await user.click(await screen.findByRole("button", { name: "원격 접속 켜기" }));
    await screen.findByText("2/4 브라우저에서 Tailscale 로그인을 마쳐 주세요");
    view.rerender(
      <PhoneConnectDialog
        open={false}
        onOpenChange={vi.fn()}
        actions={calls as unknown as RemoteSettingsActions}
        pollMs={10}
      />,
    );
    // At most the one poll already in flight finishes after the close.
    await new Promise((resolve) => setTimeout(resolve, 30));
    const settled = calls.status.mock.calls.length;
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(calls.status.mock.calls.length).toBe(settled);
    await waitFor(() => expect(calls.pairBegin).not.toHaveBeenCalled());
  });
});
