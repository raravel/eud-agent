import { describe, it, expect } from "vitest";
import { parseImageTool } from "./imageTool";

// A 1×1 transparent PNG, the shape `map_draft_render` returns.
const PNG_BASE64 =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

describe("parseImageTool", () => {
  it("turns the map image envelope into a data URL with its dimensions", () => {
    const view = parseImageTool({
      detail: JSON.stringify({
        image: { mimeType: "image/png", width: 96, height: 64, data: PNG_BASE64 },
      }),
    });
    expect(view).toEqual({
      mimeType: "image/png",
      width: 96,
      height: 64,
      src: `data:image/png;base64,${PNG_BASE64}`,
      dataBytes: null,
      metadata: null,
    });
  });

  it("keeps the other result fields as compact metadata", () => {
    const view = parseImageTool({
      detail: JSON.stringify({
        region: { x: 0, y: 0, width: 12, height: 8 },
        image: { mimeType: "image/png", width: 96, height: 64, data: PNG_BASE64 },
      }),
    });
    expect(view?.metadata).toBe('{"region":{"x":0,"y":0,"width":12,"height":8}}');
  });

  it("renders a receipt image (payload replaced by its digest) as a placeholder", () => {
    const view = parseImageTool({
      detail: JSON.stringify({
        image: {
          mimeType: "image/png",
          width: 96,
          height: 64,
          dataBytes: 1234,
          dataSha256: "ab".repeat(32),
        },
      }),
    });
    expect(view).toMatchObject({
      src: null,
      dataBytes: 1234,
      width: 96,
      height: 64,
    });
  });

  it("returns null for anything that is not an image envelope", () => {
    expect(parseImageTool({ detail: undefined })).toBeNull();
    expect(parseImageTool({ detail: "OK: units|Hit Points|0 = 20480" })).toBeNull();
    expect(parseImageTool({ detail: '{"ok":true}' })).toBeNull();
    expect(parseImageTool({ detail: '{"image":"not an object"}' })).toBeNull();
    expect(
      parseImageTool({ detail: '{"image":{"mimeType":"text/plain","data":"x"}}' }),
    ).toBeNull();
    expect(
      parseImageTool({ detail: '{"image":{"mimeType":"image/png","width":1}}' }),
    ).toBeNull();
    // Truncated JSON (cut mid-base64) cannot be rendered — keep the text row.
    expect(
      parseImageTool({
        detail: '{"image":{"mimeType":"image/png","data":"iVBORw0K …(잘림)',
      }),
    ).toBeNull();
  });
});
