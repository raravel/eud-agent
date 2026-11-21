import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { MapVaultSettingsPanel } from "@/components/MapVaultSettingsPanel";

function actions(hasKey: boolean, address = "https://sc.mi3.dev") {
  return {
    load: vi.fn().mockResolvedValue({ address, hasKey }),
    save: vi.fn(),
    clearKey: vi.fn().mockResolvedValue({ address, hasKey: false }),
  };
}

describe("MapVaultSettingsPanel", () => {
  it("saves the address and a new key, then shows only that a key is set", async () => {
    const calls = actions(false);
    calls.save.mockResolvedValue({ address: "http://localhost:5173", hasKey: true });
    render(<MapVaultSettingsPanel actions={calls} />);

    const address = await screen.findByLabelText("사이트 주소");
    expect(address).toHaveValue("https://sc.mi3.dev");
    expect(screen.getByText(/저장된 키가 없습니다/)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: "저장된 키 삭제" }),
    ).not.toBeInTheDocument();

    fireEvent.change(address, { target: { value: " http://localhost:5173 " } });
    const key = screen.getByLabelText("API 키");
    expect(key).toHaveAttribute("type", "password");
    fireEvent.change(key, { target: { value: "  mv_secretsecretsecretsecretsecretse " } });
    fireEvent.click(screen.getByRole("button", { name: "저장" }));

    await waitFor(() =>
      expect(calls.save).toHaveBeenCalledWith(
        "http://localhost:5173",
        "mv_secretsecretsecretsecretsecretse",
      ),
    );
    expect(await screen.findByRole("status")).toHaveTextContent("저장했습니다");
    // The key field is emptied: the panel never keeps or shows the key.
    expect(screen.getByLabelText("API 키")).toHaveValue("");
    expect(screen.getByLabelText("API 키")).toHaveAttribute(
      "placeholder",
      "저장된 키 사용 중 · 교체하려면 입력",
    );
    expect(screen.getByLabelText("사이트 주소")).toHaveValue("http://localhost:5173");
    expect(screen.getByRole("button", { name: "저장된 키 삭제" })).toBeEnabled();
  });

  it("keeps the stored key when the key field is left blank, and can reset the address", async () => {
    const calls = actions(true, "http://127.0.0.1:5173");
    calls.save.mockResolvedValue({ address: "https://sc.mi3.dev", hasKey: true });
    render(<MapVaultSettingsPanel actions={calls} />);

    expect(await screen.findByLabelText("사이트 주소")).toHaveValue(
      "http://127.0.0.1:5173",
    );
    fireEvent.click(screen.getByRole("button", { name: "기본 주소로" }));
    expect(screen.getByLabelText("사이트 주소")).toHaveValue("https://sc.mi3.dev");
    expect(screen.getByRole("button", { name: "기본 주소로" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    await waitFor(() =>
      expect(calls.save).toHaveBeenCalledWith("https://sc.mi3.dev", undefined),
    );
  });

  it("explains a refused address or key and deletes the key on request", async () => {
    const calls = actions(true);
    calls.save.mockRejectedValueOnce("mapvault_address_invalid");
    calls.save.mockRejectedValueOnce("mapvault_key_format");
    render(<MapVaultSettingsPanel actions={calls} />);

    const address = await screen.findByLabelText("사이트 주소");
    fireEvent.change(address, { target: { value: "http://example.com" } });
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "http는 localhost와 127.0.0.1에서만",
    );

    fireEvent.change(screen.getByLabelText("API 키"), { target: { value: "hello" } });
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("mv_로 시작하는 키"),
    );

    fireEvent.click(screen.getByRole("button", { name: "저장된 키 삭제" }));
    await waitFor(() => expect(calls.clearKey).toHaveBeenCalledTimes(1));
    expect(await screen.findByText(/저장된 키가 없습니다/)).toBeInTheDocument();
  });

  it("names the key's owner after the site checked it and warns about a read-only key", async () => {
    const calls = actions(false);
    calls.save.mockResolvedValueOnce({
      address: "https://sc.mi3.dev",
      hasKey: true,
      owner: { username: "maker1", nickname: "제작자", keyName: "agent", readOnly: true },
    });
    render(<MapVaultSettingsPanel actions={calls} />);
    fireEvent.change(await screen.findByLabelText("API 키"), {
      target: { value: "mv_secretsecretsecretsecretsecretse" },
    });
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    expect(await screen.findByText(/제작자\(maker1\)님의 키 "agent"/)).toBeInTheDocument();
    expect(screen.getByText(/읽기 전용 키입니다/)).toBeInTheDocument();
    expect(screen.getByText(/SDK 설치/)).toBeInTheDocument();
  });

  it("explains a key the site refused and a site it could not reach", async () => {
    const calls = actions(false);
    calls.save.mockRejectedValueOnce("mapvault_key_invalid");
    calls.save.mockRejectedValueOnce("mapvault_unreachable");
    render(<MapVaultSettingsPanel actions={calls} />);
    fireEvent.change(await screen.findByLabelText("API 키"), {
      target: { value: "mv_secretsecretsecretsecretsecretse" },
    });
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("이 사이트에서 쓸 수 없는 API 키");
    fireEvent.change(screen.getByLabelText("API 키"), {
      target: { value: "mv_secretsecretsecretsecretsecretse" },
    });
    fireEvent.click(screen.getByRole("button", { name: "저장" }));
    await waitFor(() =>
      expect(screen.getByRole("alert")).toHaveTextContent("사이트에 연결할 수 없습니다"),
    );
    expect(screen.queryByRole("button", { name: "저장된 키 삭제" })).not.toBeInTheDocument();
  });

  it("says so when the settings cannot be read", async () => {
    const calls = actions(false);
    calls.load.mockRejectedValue(new Error("boom"));
    render(<MapVaultSettingsPanel actions={calls} />);
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "MapVault 설정을 불러오지 못했습니다",
    );
  });
});
