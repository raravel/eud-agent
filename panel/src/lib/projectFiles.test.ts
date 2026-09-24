import { describe, expect, it, vi } from "vitest";

import {
  createProjectFileSource,
  projectFileAsFile,
  rankProjectFiles,
} from "@/lib/projectFiles";

const files = [
  { path: ".eud-agent/workspace/specs/main.md", size: 1 },
  { path: "src/lib/domain.eps", size: 2 },
  { path: "src/main.eps", size: 3 },
  { path: "maps/source.scx", size: 4 },
  { path: "src/remain.eps", size: 5 },
];

describe("rankProjectFiles", () => {
  it("ranks name prefix, name substring, then path, harness files last", () => {
    expect(
      rankProjectFiles("w", files, "MAIN", 10).map((file) => file.path),
    ).toEqual([
      "src/main.eps",
      "src/remain.eps",
      "src/lib/domain.eps",
      ".eud-agent/workspace/specs/main.md",
    ]);
    expect(rankProjectFiles("w", files, "maps/", 10)).toEqual([
      { workspaceId: "w", path: "maps/source.scx", size: 4 },
    ]);
  });

  it("lists authoring files first for an empty query and honors the limit", () => {
    expect(rankProjectFiles("w", files, "", 2).map((file) => file.path)).toEqual([
      "src/main.eps",
      "src/remain.eps",
    ]);
  });
});

describe("projectFileAsFile", () => {
  it("names the File after the path's last segment", async () => {
    const file = projectFileAsFile("src/main.eps", new Uint8Array([104, 105]));
    expect(file.name).toBe("main.eps");
    expect(await file.text()).toBe("hi");
  });
});

describe("createProjectFileSource", () => {
  it("reuses one tree scan across keystrokes and retries a failed scan", async () => {
    const list = vi
      .fn()
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValue({ project: "p", workspaceId: "w", files });
    const source = createProjectFileSource(list, vi.fn());

    await expect(source.search("main")).rejects.toThrow("offline");
    expect((await source.search("source"))[0]?.path).toBe("maps/source.scx");
    expect((await source.search("main"))[0]?.path).toBe("src/main.eps");
    expect(list).toHaveBeenCalledTimes(2);
  });

  it("reads through the file's own workspace id", async () => {
    const readBytes = vi.fn().mockResolvedValue(new Uint8Array([104]));
    const source = createProjectFileSource(vi.fn(), readBytes);
    const file = await source.read({ workspaceId: "w", path: "src/a.eps", size: 1 });
    expect(readBytes).toHaveBeenCalledWith("w", "src/a.eps");
    expect(file.name).toBe("a.eps");
  });
});
