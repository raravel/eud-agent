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
import { useTranslation } from "react-i18next";

import { ragTierLabel } from "@/components/ReferenceDocument";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import i18n from "@/lib/i18n";
import type { RagSearchHit, RagSearchResponse } from "@/lib/ipc";
import { cn } from "@/lib/utils";

export interface RagViewProps {
  onSearch(query: string): Promise<RagSearchResponse>;
  onOpen(hit: RagSearchHit): void;
  /** Id of the reference tab active in the center column (highlighted). */
  activeId?: string | null;
}

function matchLabel(kind: RagSearchHit["matchKind"]): string {
  return i18n.t(`wiki.rag.match.${kind}`, { defaultValue: kind });
}

function formatScore(hit: RagSearchHit): string {
  return hit.matchKind === "lexical"
    ? i18n.t("wiki.rag.lexicalScore", { count: Math.round(hit.score) })
    : hit.score.toFixed(3);
}

/** The preview skips the scraped `제목:` header the card already shows as its title. */
function preview(text: string): string {
  return text.replace(/^(?:\s*제목:[^\n]*\n)+/, "").replace(/\[\[\[CONTENT-ELEMENT-\d+\]\]\]/g, "").trim();
}

export function RagView({ onSearch, onOpen, activeId = null }: RagViewProps) {
  const { t } = useTranslation();
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
        t("wiki.rag.searchFailed", {
          error: cause instanceof Error ? cause.message : String(cause),
        }),
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
          placeholder={t("wiki.rag.placeholder")}
          aria-label={t("wiki.rag.queryLabel")}
          className="h-8 text-xs"
        />
        <Button type="submit" size="sm" disabled={loading || !query.trim()} aria-label={t("wiki.rag.searchLabel")}>
          {loading ? <Spinner className="size-3.5" /> : <Search className="size-3.5" aria-hidden="true" />}
          {t("wiki.rag.search")}
        </Button>
      </form>

      <div className="min-h-0 flex-1 overflow-y-auto p-2" aria-live="polite">
        {error && <p className="p-2 text-xs text-destructive">{error}</p>}
        {!error && !result && (
          <p className="p-2 text-xs text-muted-foreground">
            {t("wiki.rag.intro")}
          </p>
        )}
        {!error && result && (
          <>
            <p className="px-1 pb-2 text-[11px] text-muted-foreground">
              {result.indexSize === 0
                ? t("wiki.rag.noIndex")
                : t("wiki.rag.summary", { size: result.indexSize.toLocaleString(), count: result.hits.length })}
              {result.indexSize > 0 &&
                !result.semanticReady &&
                ` · ${t("wiki.rag.semanticPending")}`}
            </p>
            {result.indexSize > 0 && result.hits.length === 0 && (
              <p className="p-2 text-xs text-muted-foreground">{t("wiki.rag.noResults")}</p>
            )}
            <ul className="space-y-2">
              {result.hits.map((hit) => (
                <li key={hit.id}>
                  <button
                    type="button"
                    aria-current={activeId === hit.id ? "true" : undefined}
                    aria-label={t("wiki.rag.openLabel", { title: hit.title })}
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
                        {ragTierLabel(hit.tier)}
                      </Badge>
                      <Badge variant="outline" className="px-1.5 text-[10px]">
                        {matchLabel(hit.matchKind)}
                      </Badge>
                      {hit.part && (
                        <span className="text-[10px] text-muted-foreground">
                          {t("wiki.rag.part", { index: hit.part[0], total: hit.part[1] })}
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
