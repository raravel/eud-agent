/**
 * Preview-truncation helper (features/03 ## Behaviors → Review):
 *   "display truncated at 1 MiB measured AND sliced in the same UTF-16
 *    code-unit metric (vanilla advisory fix) with a notice; Apply always sends
 *    full text."
 *
 * The vanilla panel measured the limit in UTF-8 BYTES but sliced in UTF-16 code
 * units, so a string just over 1 MiB of multi-byte text was cut at the wrong
 * index. This module pins ONE metric: both the measure (`String.length`) and
 * the slice (`String.slice`) operate on UTF-16 code units, so the boundary is
 * exact and consistent. Apply never uses this — it always sends the full text.
 */

/** 1 MiB display cap, in UTF-16 code units (== JS `String.length`). */
export const PREVIEW_LIMIT = 1024 * 1024;

/** Result of {@link truncateForDisplay}. */
export interface Truncated {
  /** The (possibly sliced) text to display. */
  text: string;
  /** True iff the input exceeded the limit and was sliced. */
  truncated: boolean;
}

/**
 * Truncate `code` for display at `limit` UTF-16 code units. A string exactly at
 * the limit is NOT truncated; one code unit over is sliced to exactly `limit`.
 */
export function truncateForDisplay(
  code: string,
  limit: number = PREVIEW_LIMIT,
): Truncated {
  if (code.length <= limit) {
    return { text: code, truncated: false };
  }
  return { text: code.slice(0, limit), truncated: true };
}
