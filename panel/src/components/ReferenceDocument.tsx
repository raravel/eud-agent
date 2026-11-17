/**
 * Reference document viewer — the center-column body of one "참고 문서" tab.
 *
 * Shows the whole article a search hit belongs to (the backend joins its
 * overlapping parts) as markdown through the shared Response surface, with the
 * title, source tier and original link in the header. Every http(s) link in
 * the body opens in the system browser instead of navigating the WebView.
 */
import { useMemo, type MouseEvent } from "react";
import { ExternalLink } from "lucide-react";
import { useTranslation } from "react-i18next";

import { Response } from "@/components/ai-elements/response";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import type { RagArticle, RagTier } from "@/lib/ipc";
import i18n from "@/lib/i18n";
import { referenceMarkdown } from "@/lib/referenceMarkdown";

/** The tier's label in the current language (a tier the app does not know shows as sent). */
export function ragTierLabel(tier: RagTier): string {
  return i18n.t(`wiki.tier.${tier}`, { defaultValue: tier });
}

export interface ReferenceDocumentProps {
  /** Title known before the article loads (from the search hit). */
  title: string;
  article: RagArticle | null;
  loading: boolean;
  error: string | null;
  onOpenLink(url: string): void;
}

export function ReferenceDocument({
  title,
  article,
  loading,
  error,
  onOpenLink,
}: ReferenceDocumentProps) {
  const { t } = useTranslation();
  const markdown = useMemo(
    () => (article ? referenceMarkdown(article.text, article.title) : ""),
    [article],
  );

  const handleClick = (event: MouseEvent<HTMLDivElement>) => {
    if (!(event.target instanceof Element)) return;
    const href = event.target.closest("a[href]")?.getAttribute("href");
    if (!href || !/^https?:\/\//i.test(href)) return;
    event.preventDefault();
    onOpenLink(href);
  };

  const heading = article?.title ?? title;
  return (
    <article aria-label={t("wiki.reference.label")} className="flex h-full min-h-0 min-w-0 flex-col overflow-hidden">
      <div className="flex shrink-0 items-center gap-2 border-b border-border bg-muted/30 px-4 py-2 text-xs">
        <span className="min-w-0 flex-1 truncate font-medium" title={heading}>
          {heading}
        </span>
        {article && (
          <>
            <Badge variant="secondary" className="shrink-0">
              {ragTierLabel(article.tier)}
            </Badge>
            {article.parts > 1 && (
              <Badge variant="outline" className="shrink-0">
                {t("wiki.reference.partsMerged", { count: article.parts })}
              </Badge>
            )}
            {article.url && (
              <Button
                type="button"
                size="sm"
                variant="outline"
                className="h-7 shrink-0"
                onClick={() => onOpenLink(article.url!)}
                aria-label={t("wiki.reference.openOriginalLabel", { title: heading })}
              >
                <ExternalLink className="size-3.5" aria-hidden="true" /> {t("wiki.reference.openOriginal")}
              </Button>
            )}
          </>
        )}
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        {loading && !article ? (
          <div className="flex h-full items-center justify-center gap-2 text-sm text-muted-foreground">
            <Spinner className="size-4" />
            {t("wiki.reference.opening")}
          </div>
        ) : error ? (
          <div
            role="alert"
            className="rounded border border-destructive/40 bg-destructive/10 p-3 text-sm text-destructive"
          >
            {error}
          </div>
        ) : article ? (
          <div className="mx-auto max-w-4xl text-sm leading-7">
            {!article.complete && (
              <p role="status" className="mb-4 rounded border border-border bg-muted/30 p-2 text-xs text-muted-foreground">
                {t("wiki.reference.incomplete")}
              </p>
            )}
            <h1 className="mb-4 text-lg font-semibold leading-7">{article.title}</h1>
            <div className="[overflow-wrap:anywhere]" onClick={handleClick}>
              <Response mode="static">{markdown}</Response>
            </div>
          </div>
        ) : null}
      </div>
    </article>
  );
}
