import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import type { RemoteDevice, RemoteStatus } from "@/lib/ipc";
import { RemoteSettingsPanel, type RemoteSettingsActions } from "./RemoteSettingsPanel";

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
      serveCommand: "tailscale serve --bg --https=443 http://127.0.0.1:47821",
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
    copy: vi.fn().mockResolvedValue(undefined),
  } as never;
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
});
