import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { SessionIdCopyButton } from "./SessionIdCopyButton";

const FULL_ID = "a3f9c2d1-7b1e-4c2a-9d0f-1234567890ab";

function stubClipboard(writeText: (text: string) => Promise<void>) {
  Object.defineProperty(navigator, "clipboard", {
    value: { writeText },
    configurable: true,
  });
}

afterEach(() => {
  vi.restoreAllMocks();
  Reflect.deleteProperty(navigator, "clipboard");
});

describe("SessionIdCopyButton", () => {
  it("copies the full session id and confirms it", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    stubClipboard(writeText);
    render(<SessionIdCopyButton id={FULL_ID} name="트리거 수정" />);

    const button = screen.getByRole("button", { name: "트리거 수정 세션 ID 복사" });
    expect(button).toHaveAttribute("title", `세션 ID ${FULL_ID} 복사`);

    fireEvent.click(button);

    expect(writeText).toHaveBeenCalledWith(FULL_ID);
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent("세션 ID를 복사했습니다."),
    );
  });

  it("announces a clipboard failure with a recovery action", async () => {
    stubClipboard(vi.fn().mockRejectedValue(new Error("denied")));
    render(<SessionIdCopyButton id={FULL_ID} name="트리거 수정" />);

    fireEvent.click(screen.getByRole("button", { name: "트리거 수정 세션 ID 복사" }));

    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent(
        "세션 ID를 복사하지 못했습니다. 클립보드 접근을 허용한 뒤 다시 시도하세요.",
      ),
    );
  });
});
