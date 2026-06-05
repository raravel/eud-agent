// Vendored AI Elements "Response" (decision 06). The upstream registry ships
// Response as a thin memoized Streamdown wrapper; it is reconstructed here from
// the documented shape (the registry endpoint returns no standalone file).
//
// Streaming-safe markdown: re-renders cleanly as delta text grows. All highlighter
// / math assets are bundled from the `streamdown` npm package — never a runtime CDN
// (rules.md). The "size-full" + prose classes match the upstream defaults.
"use client";

import { cn } from "@/lib/utils";
import type { ComponentProps } from "react";
import { memo } from "react";
import { Streamdown } from "streamdown";

export type ResponseProps = ComponentProps<typeof Streamdown>;

export const Response = memo(
  ({ className, ...props }: ResponseProps) => (
    <Streamdown
      className={cn(
        "size-full [&>*:first-child]:mt-0 [&>*:last-child]:mb-0",
        className,
      )}
      {...props}
    />
  ),
  (prev, next) => prev.children === next.children,
);

Response.displayName = "Response";
