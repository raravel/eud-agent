/**
 * Reference index search — the "참고 문서" tab body of the project sidebar.
 *
 * Runs the same hybrid search the agent's `search_docs` tool runs (lexical
 * identifier/Korean-term hits first, then semantic hits once the embedding
 * model is warm) so the user can see exactly which reference chunks the agent
 * would get. Search is submit-driven (Enter / 검색) because a semantic query
 * embeds the text. Each hit shows its tier, match kind, score and a short
 * preview; selecting it opens the whole article as a center document tab.
 */
import { useState, type FormEvent } from "react";
import { FileText, Search } from "lucide-react";

import { RAG_TIER_LABELS } from "@/components/ReferenceDocument";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import type { RagSearchHit, RagSearchResponse } from "@/lib/ipc";
import { cn } from "@/lib/utils";

export interface RagViewProps {
  onSearch(query: string): Promise<RagSearchResponse>;
  onOpen(hit: RagSearchHit): void;
  /** Id of the reference tab active in the center column (highlighted). */
  activeId?: string | null;
}

const MATCH_LABELS: Record<RagSearchHit["matchKind"], string> = {
  lexical: "어휘 일치",
  semantic: "의미 유사",
};

function formatScore(hit: RagSearchHit): string {
  return hit.matchKind === "lexical" ? `${Math.round(hit.score)}회` : hit.score.toFixed(3);
}

/** The preview skips the scraped `제목:` header the card already shows as its title. */
function preview(text: string): string {
  return text.replace(/^(?:\s*제목:[^\n]*\n)+/, "").replace(/\[\[\[CONTENT-ELEMENT-\d+\]\]\]/g, "").trim();
}

export function RagView({ onSearch, onOpen, activeId = null }: RagViewProps) {
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<RagSearchResponse | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    const text = query.trim();
    if (!text || loading) return;
    setLoading(true);
    setError(null);
    try {
      setResult(await onSearch(text));
    } catch (cause) {
      setError(
        `참고 문서를 검색하지 못했습니다. 잠시 후 다시 시도해 주세요. (${
          cause instanceof Error ? cause.message : String(cause)
        })`,
      );
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <form role="search" onSubmit={submit} className="flex gap-2 border-b border-border p-2">
        <Input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="예: chatEvent, 유닛 생성 트리거"
          aria-label="참고 문서 검색어"
          className="h-8 text-xs"
        />
        <Button type="submit" size="sm" disabled={loading || !query.trim()} aria-label="참고 문서 검색">
          {loading ? <Spinner className="size-3.5" /> : <Search className="size-3.5" aria-hidden="true" />}
          검색
        </Button>
      </form>

      <div className="min-h-0 flex-1 overflow-y-auto p-2" aria-live="polite">
        {error && <p className="p-2 text-xs text-destructive">{error}</p>}
        {!error && !result && (
          <p className="p-2 text-xs text-muted-foreground">
            에이전트가 search_docs로 받는 것과 같은 참고 문서 결과를 직접 검색해 볼 수 있습니다. 결과를 누르면 가운데에 문서로 열립니다.
          </p>
        )}
        {!error && result && (
          <>
            <p className="px-1 pb-2 text-[11px] text-muted-foreground">
              {result.indexSize === 0
                ? "참고 문서 인덱스가 설치되지 않았습니다. 설정에서 RAG 자산을 받아 주세요."
                : `인덱스 ${result.indexSize.toLocaleString()}개 조각 중 ${result.hits.length}개 결과`}
              {result.indexSize > 0 &&
                !result.semanticReady &&
                " · 의미 검색 모델 준비 중이라 어휘 일치 결과만 표시합니다"}
            </p>
            {result.indexSize > 0 && result.hits.length === 0 && (
              <p className="p-2 text-xs text-muted-foreground">일치하는 참고 문서가 없습니다.</p>
            )}
            <ul className="space-y-2">
              {result.hits.map((hit) => (
                <li key={hit.id}>
                  <button
                    type="button"
                    aria-current={activeId === hit.id ? "true" : undefined}
                    aria-label={`${hit.title} 문서 열기`}
                    onClick={() => onOpen(hit)}
                    className={cn(
                      "w-full rounded-md border bg-card/40 p-2 text-left outline-none transition-colors hover:bg-accent/40 focus-visible:ring-2 focus-visible:ring-ring",
                      activeId === hit.id ? "border-primary" : "border-border",
                    )}
                  >
                    <span className="flex items-start gap-1.5">
                      <FileText className="mt-0.5 size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
                      <span className="min-w-0 flex-1 truncate text-xs font-medium" title={hit.title}>
                        {hit.title}
                      </span>
                    </span>
                    <span className="mt-1 flex flex-wrap items-center gap-1 pl-5">
                      <Badge variant="secondary" className="px-1.5 text-[10px]">
                        {RAG_TIER_LABELS[hit.tier] ?? hit.tier}
                      </Badge>
                      <Badge variant="outline" className="px-1.5 text-[10px]">
                        {MATCH_LABELS[hit.matchKind] ?? hit.matchKind}
                      </Badge>
                      {hit.part && (
                        <span className="text-[10px] text-muted-foreground">
                          조각 {hit.part[0]}/{hit.part[1]}
                        </span>
                      )}
                      <span className="text-[10px] text-muted-foreground">{formatScore(hit)}</span>
                    </span>
                    <span className="mt-1 line-clamp-3 whitespace-pre-wrap pl-5 [overflow-wrap:anywhere] text-xs text-muted-foreground">
                      {preview(hit.text)}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
    </div>
  );
}
