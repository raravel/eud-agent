/**
 * Research artifact card (features/staged-workflow-plan.md ## Phase 2 Panel):
 * a collapsible card showing the research summary and the project-relative
 * artifact path once `WorkflowEvent.research` first appears. The store also
 * archives the summary into the log; this card is the live inline surface.
 */
import { ChevronDown, Search } from "lucide-react";
import { useState } from "react";

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import type { WorkflowArtifactRef } from "@/lib/ipc";

export interface ResearchCardProps {
  research: WorkflowArtifactRef;
  defaultOpen?: boolean;
}

export function ResearchCard({ research, defaultOpen = false }: ResearchCardProps) {
  const [open, setOpen] = useState(defaultOpen);

  return (
    <section aria-label="조사 결과" className="shrink-0 border-t border-border px-4 py-2">
      <Collapsible open={open} onOpenChange={setOpen}>
        <Card className="gap-0 overflow-hidden border-border bg-card/60 py-0 shadow-none">
          <CardHeader className="gap-0 px-4 py-2.5">
            <CollapsibleTrigger
              className="group flex w-full items-center gap-3 text-left outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
              aria-label={open ? "조사 결과 접기" : "조사 결과 펼치기"}
            >
              <Search className="size-4 shrink-0 text-primary" aria-hidden="true" />
              <CardTitle className="min-w-0 flex-1 truncate text-sm">
                조사 결과
              </CardTitle>
              <ChevronDown
                aria-hidden="true"
                className="size-4 shrink-0 text-muted-foreground transition-transform duration-200 group-data-[state=open]:rotate-180 motion-reduce:transition-none"
              />
            </CollapsibleTrigger>
          </CardHeader>
          <CollapsibleContent>
            <CardContent className="border-t border-border px-4 py-3">
              <p className="whitespace-pre-wrap text-sm leading-6 text-foreground">
                {research.summary}
              </p>
              <p className="mt-2 break-all font-mono text-[11px] text-muted-foreground">
                {research.path}
              </p>
            </CardContent>
          </CollapsibleContent>
        </Card>
      </Collapsible>
    </section>
  );
}
