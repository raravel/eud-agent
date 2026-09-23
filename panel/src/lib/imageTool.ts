/**
 * Parse the map image envelope a rendering tool (`map_draft_render`,
 * `map_task_render`, …) returns as its tool_result text:
 *
 *   {"image":{"mimeType":"image/png","width":W,"height":H,"data":"<base64>"}}
 *
 * The panel shows that as the picture itself instead of a wall of base64. A
 * result replayed from a run receipt keeps the metadata but carries only the
 * PNG's byte count/SHA-256 (`dataBytes`/`dataSha256`); that becomes a labeled
 * placeholder. Anything else keeps the JSON result row.
 */
import type { AgentTool } from "@/components/AgentStream";

export interface ImageToolView {
  mimeType: string;
  width: number | null;
  height: number | null;
  /** `data:` URL for the `<img>`, or null when only the digest survived. */
  src: string | null;
  /** Base64 length recorded by the receipt when the payload was dropped. */
  dataBytes: number | null;
  /** Compact JSON of the other result fields (minus the image), if any. */
  metadata: string | null;
}

function dimension(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : null;
}

export function parseImageTool(
  tool: Pick<AgentTool, "detail">,
): ImageToolView | null {
  if (!tool.detail) return null;
  // The envelope is compact JSON starting with the `image` object; skip the
  // parse (which can be megabytes) for every result that cannot be one.
  if (!tool.detail.startsWith("{") || !tool.detail.includes('"image"')) {
    return null;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(tool.detail);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
  const result = parsed as Record<string, unknown>;
  const image = result.image;
  if (!image || typeof image !== "object" || Array.isArray(image)) return null;
  const { mimeType, width, height, data, dataBytes } = image as Record<
    string,
    unknown
  >;
  if (typeof mimeType !== "string" || !mimeType.startsWith("image/")) {
    return null;
  }
  const hasData = typeof data === "string" && data.length > 0;
  const omittedBytes = dimension(dataBytes);
  if (!hasData && omittedBytes === null) return null;

  const rest = Object.entries(result).filter(([key]) => key !== "image");
  return {
    mimeType,
    width: dimension(width),
    height: dimension(height),
    src: hasData ? `data:${mimeType};base64,${data}` : null,
    dataBytes: hasData ? null : omittedBytes,
    metadata: rest.length > 0 ? JSON.stringify(Object.fromEntries(rest)) : null,
  };
}
