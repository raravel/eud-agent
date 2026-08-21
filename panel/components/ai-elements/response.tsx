// Vendored AI Elements "Response" (decision 06). The upstream registry ships
// Response as a thin memoized Streamdown wrapper; it is reconstructed here from
// the documented shape (the registry endpoint returns no standalone file).
//
// Streaming-safe markdown: re-renders cleanly as delta text grows. All highlighter
// / math assets are bundled from the `streamdown` npm package — never a runtime CDN
// (rules.md). The "size-full" + prose classes match the upstream defaults.
"use client";
import { cn } from "@/lib/utils";
import type { DiagramPlugin } from "@streamdown/mermaid";
import type { ComponentProps } from "react";
import { memo, useEffect, useState } from "react";
import { Streamdown } from "streamdown";

export type ResponseProps = ComponentProps<typeof Streamdown>;
type DiagramPlugins = { mermaid: DiagramPlugin };
const DIAGRAM_CONTROLS = { mermaid: { panZoom: false } } as const;
let diagramPlugins: DiagramPlugins | null = null;
let diagramPluginsPromise: Promise<DiagramPlugins | null> | null = null;
export const Response = memo(
  ({ className, ...props }: ResponseProps) => (
    <Streamdown
      className={cn(
        "size-full [&>*:first-child]:mt-0 [&>*:last-child]:mb-0",
        className,
      )}
      // Citation links (EUD-090): streamdown's default linkSafety renders every
      // markdown link as an href-LESS button + confirm modal — in the panel the
      // agent's evidence citations looked like dead text. Disabled so links are
      // plain <a href target="_blank">; the WebView2 host (bridge) routes the
      // resulting NewWindowRequested to the user's default browser, so the
      // panel itself never navigates away. Callers can still override via props.
      linkSafety={{ enabled: false }}
      {...props}
    />
  ),
  (prev, next) => prev.children === next.children,
);

function DiagramResponseContent(props: ResponseProps) {
  const hasMermaidFence =
    typeof props.children === "string" &&
    /(?:^|\n)[ \t]{0,3}(?:`{3,}|~{3,})mermaid(?:[ \t]*\n|[ \t]*$)/i.test(
      props.children,
    );
  const [plugins, setPlugins] = useState<DiagramPlugins | null>(diagramPlugins);

  useEffect(() => {
    if (!hasMermaidFence || plugins !== null) return;
    let active = true;
    diagramPluginsPromise ??= import("@streamdown/mermaid")
      .then(({ mermaid }) => {
        diagramPlugins = { mermaid };
        return diagramPlugins;
      })
      .catch((error: unknown) => {
        console.error("Failed to load bundled Mermaid renderer", error);
        return null;
      });
    void diagramPluginsPromise.then((loaded) => {
      if (active && loaded !== null) setPlugins(loaded);
    });
    return () => {
      active = false;
    };
  }, [hasMermaidFence, plugins]);

  if (!hasMermaidFence) return <Response {...props} />;

  return (
    <Response
      key={plugins === null ? "markdown" : "mermaid"}
      {...props}
      controls={DIAGRAM_CONTROLS}
      plugins={plugins ?? undefined}
    />
  );
}

/** Streamdown response with lazily loaded, bundled Mermaid rendering. */
export const DiagramResponse = memo(
  DiagramResponseContent,
  (prev, next) => prev.children === next.children,
);

DiagramResponse.displayName = "DiagramResponse";

Response.displayName = "Response";
