import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import RemoteApp from "./RemoteApp";
import type { FetchFn } from "./remoteApi";

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

afterEach(() => {
  window.history.replaceState(null, "", "/");
});

describe("RemoteApp", () => {
  it("pairs with the code from the QR link and clears it from the address", async () => {
    window.history.replaceState(null, "", "/remote/#pair=abcd2345");
    const user = userEvent.setup();
    const fetchFn = vi.fn<FetchFn>(async (input, init) => {
      if (input === "/remote/api/me") return json(401, { error: "no" });
      if (input === "/remote/api/pair") {
        expect(init?.method).toBe("POST");
        expect(JSON.parse(String(init?.body))).toEqual({
          code: "ABCD2345",
          deviceName: "휴대폰",
        });
        return json(200, { deviceId: "d1", name: "휴대폰" });
      }
      throw new Error(`unexpected ${input}`);
    });
    render(<RemoteApp fetchFn={fetchFn} />);

    expect(await screen.findByLabelText("연결 코드")).toHaveValue("ABCD2345");
    await user.click(screen.getByRole("button", { name: "연결" }));
    expect(await screen.findByText("연결됨")).toBeInTheDocument();
    expect(window.location.hash).toBe("");
  });

  it("shows the backend's refusal", async () => {
    const user = userEvent.setup();
    const fetchFn = vi.fn<FetchFn>(async (input) =>
      input === "/remote/api/me"
        ? json(401, { error: "no" })
        : json(401, { error: "연결 코드가 올바르지 않거나 만료되었습니다." }),
    );
    render(<RemoteApp fetchFn={fetchFn} />);

    await user.type(await screen.findByLabelText("연결 코드"), "WXYZ-2345");
    await user.click(screen.getByRole("button", { name: "연결" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "연결 코드가 올바르지 않거나 만료되었습니다.",
    );
  });

  it("recognizes a paired browser and disconnects it", async () => {
    const user = userEvent.setup();
    const fetchFn = vi.fn<FetchFn>(async (input) =>
      input === "/remote/api/me"
        ? json(200, { deviceId: "d1", name: "내 폰" })
        : json(200, { ok: true }),
    );
    render(<RemoteApp fetchFn={fetchFn} />);

    expect(await screen.findByText("내 폰")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "연결 해제" }));
    await waitFor(() =>
      expect(fetchFn).toHaveBeenCalledWith(
        "/remote/api/logout",
        expect.objectContaining({ method: "POST" }),
      ),
    );
    expect(await screen.findByLabelText("연결 코드")).toBeInTheDocument();
  });
});
