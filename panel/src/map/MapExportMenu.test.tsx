import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const protocol = vi.hoisted(() => ({
  mapExportImage: vi.fn(),
  mapExportImageSave: vi.fn(),
}));
vi.mock("./mapProtocol", () => protocol);

import { MapExportMenu } from "./MapExportMenu";

const command = { sessionId: "map-session", view: "candidate" as const };
const clipboardWrite = vi.fn();

class FakeClipboardItem {
  constructor(readonly items: Record<string, Promise<Blob>>) {}
}

beforeEach(() => {
  protocol.mapExportImage.mockReset();
  protocol.mapExportImageSave.mockReset();
  clipboardWrite.mockReset();
  vi.stubGlobal("ClipboardItem", FakeClipboardItem);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

async function choose(item: string) {
  const user = userEvent.setup();
  // userEvent installs its own clipboard; the component must reach this one.
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { write: clipboardWrite },
  });
  await user.click(screen.getByRole("button", { name: "이미지 내보내기" }));
  await user.click(await screen.findByRole("menuitem", { name: item }));
}

describe("MapExportMenu", () => {
  it("copies the whole-map PNG of the shown view to the clipboard", async () => {
    const png = new Blob(["png"], { type: "image/png" });
    protocol.mapExportImage.mockResolvedValue(png);
    clipboardWrite.mockResolvedValue(undefined);
    render(<MapExportMenu command={command} />);

    await choose("클립보드에 복사");

    expect(await screen.findByRole("status")).toHaveTextContent("클립보드에 복사했습니다");
    expect(protocol.mapExportImage).toHaveBeenCalledWith(command);
    const [[items]] = clipboardWrite.mock.calls as [[FakeClipboardItem[]]];
    await expect(items[0].items["image/png"]).resolves.toBe(png);
  });

  it("reports a clipboard failure with a recovery action", async () => {
    protocol.mapExportImage.mockResolvedValue(new Blob(["png"]));
    clipboardWrite.mockRejectedValue(new Error("Document is not focused"));
    render(<MapExportMenu command={command} />);

    await choose("클립보드에 복사");

    expect(await screen.findByRole("alert")).toHaveTextContent("PNG로 저장해 주세요");
  });

  it("saves through the native dialog and shows the saved path", async () => {
    protocol.mapExportImageSave.mockResolvedValue("C:\\out\\demo.png");
    render(<MapExportMenu command={command} />);

    await choose("PNG로 저장…");

    expect(await screen.findByRole("status")).toHaveTextContent("C:\\out\\demo.png");
    expect(protocol.mapExportImageSave).toHaveBeenCalledWith(command);
  });

  it("says nothing when the save dialog is closed", async () => {
    protocol.mapExportImageSave.mockResolvedValue(null);
    render(<MapExportMenu command={command} />);

    await choose("PNG로 저장…");

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "이미지 내보내기" })).toBeEnabled(),
    );
    expect(screen.queryByRole("status")).toBeNull();
    expect(screen.queryByRole("alert")).toBeNull();
  });
});
