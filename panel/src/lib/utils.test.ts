import { describe, expect, it } from "vitest";
import { formatPathForDisplay } from "./utils";

describe("formatPathForDisplay", () => {
  it("shows readable drive and network paths without losing their roots", () => {
    expect(formatPathForDisplay(String.raw`\\?\E:\proj\eud\proj1\native`))
      .toBe(String.raw`E:\proj\eud\proj1\native`);
    expect(formatPathForDisplay(String.raw`\\?\UNC\server\공유 폴더\project`))
      .toBe(String.raw`\\server\공유 폴더\project`);
  });

  it("preserves ordinary paths and device namespaces rather than rewriting identity", () => {
    expect(formatPathForDisplay(String.raw`\\server\공유 폴더\project`))
      .toBe(String.raw`\\server\공유 폴더\project`);
    expect(formatPathForDisplay(String.raw`\\?\Volume{1234}\project`))
      .toBe(String.raw`\\?\Volume{1234}\project`);
  });
});
