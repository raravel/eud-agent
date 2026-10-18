import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type {
  RemoteDevice,
  RemotePairingNotice,
  RemoteStatus,
  TailscaleInstallProgress,
} from "@/lib/ipc";
import {
  RemoteSettingsPanel,
  TAILSCALE_DOWNLOAD_URL,
  type RemoteSettingsActions,
} from "./RemoteSettingsPanel";

function status(overrides: Partial<RemoteStatus> = {}): RemoteStatus {
  return {
    enabled: false,
    port: 47821,
    listening: false,
    error: null,
    tailscale: {
      installed: true,
      running: true,
      dnsName: "pc.tail1234.ts.net",
      serveConfigured: false,
      funnel: false,
      serveCommand: "tailscale serve --bg --https=443 http://127.0.0.1:47821",
      backendState: "Running",
      installSupported: true,
    },
    ...overrides,
  };
}

function actions(
  initial: RemoteStatus,
  devices: RemoteDevice[] = [],
): RemoteSettingsActions & { [K in keyof RemoteSettingsActions]: ReturnType<typeof vi.fn> } {
  return {
    status: vi.fn().mockResolvedValue(initial),
    save: vi.fn(async (enabled: boolean, port: number) =>
      status({ enabled, port, listening: enabled }),
    ),
    pairBegin: vi.fn().mockResolvedValue({
      code: "ABCD2345",
      expiresAt: Date.now() + 5 * 60 * 1000,
      url: "https://pc.tail1234.ts.net/remote/#pair=ABCD2345",
    }),
    devices: vi.fn().mockResolvedValue(devices),
    revoke: vi.fn().mockResolvedValue([]),
    serve: vi.fn().mockResolvedValue({
      ...initial,
      tailscale: { ...initial.tailscale, serveConfigured: true },
    }),
    install: vi.fn(),
    login: vi.fn(),
    openUrl: vi.fn().mockResolvedValue(undefined),
    copy: vi.fn().mockResolvedValue(undefined),
  } as never;
}

function withTailscale(overrides: Partial<RemoteStatus["tailscale"]>): RemoteStatus {
  const base = status({ enabled: true, listening: true });
  return { ...base, tailscale: { ...base.tailscale, ...overrides } };
}

describe("RemoteSettingsPanel", () => {
  it("warns, toggles the server on and saves a new port", async () => {
    const user = userEvent.setup();
    const calls = actions(status());
    render(<RemoteSettingsPanel actions={calls} />);

    expect(
      await screen.findByText("연결한 기기는 이 PC에서 에이전트를 실행할 수 있습니다"),
    ).toBeInTheDocument();
    await user.click(await screen.findByRole("switch", { name: "원격 접속 허용" }));
    await waitFor(() => expect(calls.save).toHaveBeenCalledWith(true, 47821));
    expect(await screen.findByText("127.0.0.1:47821에서 대기 중")).toBeInTheDocument();

    const port = screen.getByLabelText("포트");
    await user.clear(port);
    await user.type(port, "80");
    expect(screen.getByText("1024부터 65535 사이의 숫자를 입력해 주세요.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "포트 저장" })).toBeDisabled();
    await user.clear(port);
    await user.type(port, "50000");
    await user.click(screen.getByRole("button", { name: "포트 저장" }));
    await waitFor(() => expect(calls.save).toHaveBeenLastCalledWith(true, 50000));
  });

  it("shows a pairing code with a QR code and copies the serve command", async () => {
    const user = userEvent.setup();
    const calls = actions(status({ enabled: true, listening: true }));
    render(<RemoteSettingsPanel actions={calls} />);

    await user.click(await screen.findByRole("button", { name: "공개 명령 복사" }));
    expect(calls.copy).toHaveBeenCalledWith(
      "tailscale serve --bg --https=443 http://127.0.0.1:47821",
    );

    await user.click(screen.getByRole("button", { name: "Tailscale로 공개" }));
    expect(await screen.findByRole("button", { name: "공개됨" })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "기기 연결" }));
    expect(await screen.findByText("ABCD-2345")).toBeInTheDocument();
    expect(
      await screen.findByAltText("휴대폰 카메라로 찍으면 연결 화면이 열리는 QR 코드"),
    ).toHaveAttribute("src", expect.stringMatching(/^data:image\/svg\+xml;utf8,/));
  });

  it("revokes a device", async () => {
    const user = userEvent.setup();
    const calls = actions(status({ enabled: true, listening: true }), [
      { id: "d1", name: "내 폰", createdAt: 1, lastSeenAt: null },
    ]);
    render(<RemoteSettingsPanel actions={calls} />);

    const list = await screen.findByRole("list", { name: "연결된 기기 목록" });
    await user.click(within(list).getByRole("button", { name: "내 폰 연결 해제" }));
    expect(calls.revoke).toHaveBeenCalledWith("d1");
    expect(await screen.findByText("연결된 기기가 없습니다.")).toBeInTheDocument();
  });

  it("reports a bind failure and blocks pairing while off", async () => {
    const calls = actions(
      status({ error: "포트 47821을(를) 열 수 없습니다(사용 중)." }),
    );
    render(<RemoteSettingsPanel actions={calls} />);
    expect(
      await screen.findByText("포트 47821을(를) 열 수 없습니다(사용 중)."),
    ).toHaveAttribute("role", "alert");
    expect(screen.getByRole("button", { name: "기기 연결" })).toBeDisabled();
  });

  it("warns while Funnel exposes the port and marks an expired device", async () => {
    const base = status({ enabled: true, listening: true });
    const calls = actions(
      { ...base, tailscale: { ...base.tailscale, serveConfigured: true, funnel: true } },
      [
        {
          id: "d1",
          name: "옛 폰",
          createdAt: 1,
          lastSeenAt: 1,
          ownerLogin: "me@example.com",
          expired: true,
        },
      ],
    );
    render(<RemoteSettingsPanel actions={calls} />);
    expect(
      await screen.findByText("Funnel이 이 포트를 공개 인터넷에 열어 두었습니다"),
    ).toBeInTheDocument();
    expect(screen.getByText(/만료됨/)).toBeInTheDocument();
    expect(screen.getByText("me@example.com")).toBeInTheDocument();
  });

  it("installs Tailscale with download progress, then offers the sign-in", async () => {
    const user = userEvent.setup();
    const missing = withTailscale({
      installed: false,
      running: false,
      dnsName: null,
      backendState: null,
    });
    const calls = actions(missing);
    let report: ((progress: TailscaleInstallProgress) => void) | undefined;
    let finish: (() => void) | undefined;
    const installed = withTailscale({ running: false, dnsName: null, backendState: "NeedsLogin" });
    calls.install.mockImplementation(
      () =>
        new Promise((resolve) => {
          finish = () => resolve({ rebootRequired: false, status: installed });
        }),
    );
    render(
      <RemoteSettingsPanel
        actions={{
          ...calls,
          onInstallProgress: (listener) => {
            report = listener;
            return () => {
              report = undefined;
            };
          },
        }}
      />,
    );

    await user.click(await screen.findByRole("button", { name: "Tailscale 설치" }));
    expect(calls.install).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Tailscale 설치" })).toBeDisabled();
    act(() => report?.({ phase: "download", downloaded: 18 * 1024 * 1024, total: 36 * 1024 * 1024 }));
    expect(await screen.findByText("내려받는 중 18.0 / 36.0MB")).toBeInTheDocument();
    act(() => report?.({ phase: "install" }));
    expect(await screen.findByText(/관리자 권한 확인 창이 뜨면 예를/)).toBeInTheDocument();

    act(() => finish?.());
    expect(await screen.findByText(/Tailscale을 설치했습니다/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Tailscale 로그인" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Tailscale 설치" })).not.toBeInTheDocument();
  });

  it("opens the sign-in page and follows Tailscale until it is up", async () => {
    const user = userEvent.setup();
    const needsLogin = withTailscale({ running: false, dnsName: null, backendState: "NeedsLogin" });
    const calls = actions(needsLogin);
    const url = "https://login.tailscale.com/a/abc";
    calls.login.mockResolvedValue({ url, status: needsLogin });
    render(<RemoteSettingsPanel actions={calls} />);

    await user.click(await screen.findByRole("button", { name: "Tailscale 로그인" }));
    expect(calls.openUrl).toHaveBeenCalledWith(url);
    // Polling replaces the status but never a port being typed.
    const port = screen.getByLabelText("포트");
    await user.clear(port);
    await user.type(port, "50000");
    expect(
      await screen.findByText("브라우저에서 Tailscale 로그인을 마치면 이 화면이 자동으로 이어집니다."),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "로그인 페이지 다시 열기" }));
    expect(calls.openUrl).toHaveBeenLastCalledWith(url);

    calls.status.mockResolvedValue(withTailscale({}));
    expect(
      await screen.findByText("Tailscale에 로그인했습니다.", undefined, { timeout: 4000 }),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Tailscale로 공개" })).toBeEnabled();
    expect(screen.getByLabelText("포트")).toHaveValue("50000");
  });

  it("brings a signed-in but stopped Tailscale back up", async () => {
    const user = userEvent.setup();
    const stopped = withTailscale({ running: false, backendState: "Stopped" });
    const calls = actions(stopped);
    calls.login.mockResolvedValue({ url: null, status: withTailscale({}) });
    render(<RemoteSettingsPanel actions={calls} />);

    await user.click(await screen.findByRole("button", { name: "Tailscale 연결" }));
    expect(await screen.findByText("Tailscale이 연결되었습니다.")).toBeInTheDocument();
    expect(calls.openUrl).not.toHaveBeenCalled();
  });

  it("keeps watching a Tailscale that is still starting after tailscale up", async () => {
    const user = userEvent.setup();
    const stopped = withTailscale({ running: false, backendState: "Stopped" });
    const calls = actions(stopped);
    calls.login.mockResolvedValue({
      url: null,
      status: withTailscale({ running: false, backendState: "Starting" }),
    });
    render(<RemoteSettingsPanel actions={calls} />);

    await user.click(await screen.findByRole("button", { name: "Tailscale 연결" }));
    calls.status.mockResolvedValue(withTailscale({}));
    expect(
      await screen.findByText("Tailscale이 연결되었습니다.", undefined, { timeout: 4000 }),
    ).toBeInTheDocument();
  });

  it("points to the download page where the app cannot install Tailscale", async () => {
    const user = userEvent.setup();
    const calls = actions(
      withTailscale({ installed: false, running: false, installSupported: false }),
    );
    render(<RemoteSettingsPanel actions={calls} />);
    await user.click(await screen.findByRole("button", { name: "Tailscale 내려받기 페이지 열기" }));
    expect(calls.openUrl).toHaveBeenCalledWith(TAILSCALE_DOWNLOAD_URL);
    expect(screen.queryByRole("button", { name: "Tailscale 설치" })).not.toBeInTheDocument();
  });

  it("shows a newly paired device as soon as the app announces it", async () => {
    let announce: ((notice: RemotePairingNotice) => void) | undefined;
    const calls = actions(status({ enabled: true, listening: true }));
    const paired: RemoteDevice = {
      id: "d2",
      name: "새 폰",
      createdAt: 2,
      lastSeenAt: 2,
      ownerLogin: null,
      expired: false,
    };
    const withPairing = {
      ...calls,
      onPairing: (listener: (notice: RemotePairingNotice) => void) => {
        announce = listener;
        return () => {
          announce = undefined;
        };
      },
    };
    render(<RemoteSettingsPanel actions={withPairing} />);
    await screen.findByText("연결된 기기가 없습니다.");
    calls.devices.mockResolvedValue([paired]);
    announce?.({ kind: "paired", deviceId: "d2", name: "새 폰" });
    expect(await screen.findByText("새 폰")).toBeInTheDocument();
  });
});
