//! The panel's read-only view of the reference (RAG) index.
//!
//! `rag_search` runs the same hybrid search `search_docs` gives the model, so the
//! user sees exactly which chunks the agent would get. `rag_article` reassembles
//! the article a hit belongs to: the indexer splits long articles into
//! overlapping `(part i/n)` chunks that share one citation link, and the viewer
//! shows them as one document with the overlaps removed.

use std::sync::Arc;

use serde::Serialize;

use crate::rag::{IndexEntry, Rag, MAX_TOP_K};
use crate::tool_exec::{format_doc_id, tier_label};

/// The process-wide reference index, managed for the panel.
pub struct RagManaged(pub Arc<Rag>);

/// A chunk's citation header `[title](url)`, optionally followed by ` (part i/n)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedSource {
    pub title: String,
    pub url: Option<String>,
    pub part: Option<(u32, u32)>,
}

pub(crate) fn parse_source(source: &str) -> ParsedSource {
    let source = source.trim();
    let (link, part) = split_part_suffix(source);
    // The URL itself may contain parentheses (eud-book anchors), so split at the
    // last `](` and require the link to close at the very end.
    if let (Some(rest), Some(split)) = (link.strip_prefix('['), link.rfind("](")) {
        if split > 0 && link.ends_with(')') {
            let url = &link[split + 2..link.len() - 1];
            if url.starts_with("http://") || url.starts_with("https://") {
                let title = &rest[..split - 1];
                return ParsedSource {
                    title: if title.is_empty() { url } else { title }.to_string(),
                    url: Some(url.to_string()),
                    part,
                };
            }
        }
    }
    ParsedSource {
        title: source.to_string(),
        url: None,
        part: None,
    }
}

fn split_part_suffix(source: &str) -> (&str, Option<(u32, u32)>) {
    let parsed = source
        .strip_suffix(')')
        .and_then(|body| body.rsplit_once(" (part "))
        .and_then(|(link, numbers)| {
            let (index, total) = numbers.split_once('/')?;
            let index: u32 = index.parse().ok()?;
            let total: u32 = total.parse().ok()?;
            (index >= 1 && index <= total).then_some((link, (index, total)))
        });
    match parsed {
        Some((link, part)) => (link, Some(part)),
        None => (source, None),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagSearchHit {
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    /// `[index, total]` when the hit is one part of a split article.
    pub part: Option<(u32, u32)>,
    pub tier: &'static str,
    pub match_kind: &'static str,
    pub score: f32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagSearchResponse {
    pub query: String,
    /// Number of chunks in the loaded index (0 = index asset not installed).
    pub index_size: usize,
    /// Whether the embedder is loaded; before that only lexical hits appear.
    pub semantic_ready: bool,
    pub hits: Vec<RagSearchHit>,
}

/// Run the agent's hybrid reference search for the user. A blank query returns no hits.
pub fn rag_search_payload(rag: &Rag, query: &str) -> RagSearchResponse {
    let query = query.trim();
    let hits = if query.is_empty() || rag.is_empty() {
        Vec::new()
    } else {
        rag.search_hybrid(query, MAX_TOP_K)
    };
    RagSearchResponse {
        query: query.to_string(),
        index_size: rag.len(),
        semantic_ready: rag.is_ready(),
        hits: hits
            .into_iter()
            .map(|hit| {
                let source = parse_source(&hit.source);
                RagSearchHit {
                    id: format_doc_id(hit.id),
                    title: source.title,
                    url: source.url,
                    part: source.part,
                    tier: tier_label(hit.tier_level),
                    match_kind: hit.match_kind.as_str(),
                    score: hit.score,
                    text: hit.text,
                }
            })
            .collect(),
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RagArticle {
    pub id: String,
    pub title: String,
    pub url: Option<String>,
    pub tier: &'static str,
    /// Number of chunks joined into `text` (1 for an unsplit chunk).
    pub parts: u32,
    /// False when the article's parts are missing or ambiguous in the index, so
    /// `text` is only the requested chunk.
    pub complete: bool,
    pub text: String,
}

/// Longest overlap searched between consecutive parts, in chars.
const MAX_PART_OVERLAP: usize = 2_000;
/// Shorter shared runs are coincidence, not the indexer's chunk overlap.
const MIN_PART_OVERLAP: usize = 16;

/// Append `next` to `text`, dropping the prefix of `next` that repeats the end of `text`.
fn join_overlapping(text: &mut String, next: &str) {
    let tail_start = text
        .char_indices()
        .rev()
        .nth(MAX_PART_OVERLAP.saturating_sub(1))
        .map_or(0, |(index, _)| index);
    let tail = &text[tail_start..];
    let overlap = tail
        .char_indices()
        .map(|(index, _)| &tail[index..])
        .find(|suffix| suffix.chars().count() >= MIN_PART_OVERLAP && next.starts_with(suffix))
        .map_or(0, str::len);
    if overlap == 0 && !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&next[overlap..]);
}

/// The whole article chunk `id` belongs to, or `None` for an unknown id.
pub fn rag_article_payload(rag: &Rag, id: u64) -> Option<RagArticle> {
    let entry = rag.document(id)?;
    let source = parse_source(&entry.source);
    let single = |complete: bool| RagArticle {
        id: format_doc_id(entry.id),
        title: source.title.clone(),
        url: source.url.clone(),
        tier: tier_label(entry.tier_level),
        parts: 1,
        complete,
        text: entry.text.clone(),
    };
    let (Some(url), Some((_, total))) = (&source.url, source.part) else {
        return Some(single(true));
    };

    let mut parts: Vec<(u32, &IndexEntry)> = rag
        .entries()
        .iter()
        .filter_map(|candidate| {
            let parsed = parse_source(&candidate.source);
            match (parsed.url, parsed.part) {
                (Some(candidate_url), Some((index, candidate_total)))
                    if &candidate_url == url && candidate_total == total =>
                {
                    Some((index, candidate))
                }
                _ => None,
            }
        })
        .collect();
    parts.sort_by_key(|(index, candidate)| (*index, candidate.id));
    // Two posts can share one link; only an exact 1..=total set is one article.
    let exact = parts.len() == total as usize
        && parts
            .iter()
            .enumerate()
            .all(|(position, (index, _))| *index == position as u32 + 1);
    if !exact {
        return Some(single(false));
    }

    let mut text = String::new();
    for (_, part) in &parts {
        join_overlapping(&mut text, &part.text);
    }
    Some(RagArticle {
        id: format_doc_id(entry.id),
        title: source.title,
        url: source.url,
        tier: tier_label(entry.tier_level),
        parts: total,
        complete: true,
        text,
    })
}

/// Search the reference index from the panel's "참고 문서" tab.
#[tauri::command]
pub async fn rag_search(
    state: tauri::State<'_, RagManaged>,
    query: String,
) -> Result<RagSearchResponse, String> {
    let rag = Arc::clone(&state.0);
    tauri::async_runtime::spawn_blocking(move || rag_search_payload(&rag, &query))
        .await
        .map_err(|error| format!("reference search task failed: {error}"))
}

/// Open the whole article a search hit belongs to.
#[tauri::command]
pub async fn rag_article(
    state: tauri::State<'_, RagManaged>,
    id: String,
) -> Result<RagArticle, String> {
    let rag = Arc::clone(&state.0);
    let parsed = u64::from_str_radix(id.trim(), 16)
        .map_err(|_| format!("reference document id '{id}' is not a hexadecimal id"))?;
    tauri::async_runtime::spawn_blocking(move || rag_article_payload(&rag, parsed))
        .await
        .map_err(|error| format!("reference document task failed: {error}"))?
        .ok_or_else(|| format!("reference document '{id}' is not in the index"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: u64, tier_level: u8, source: &str, text: &str) -> IndexEntry {
        IndexEntry {
            id,
            vector: vec![0.0; crate::rag::EMBED_DIM],
            tier_level,
            text: text.to_string(),
            source: source.to_string(),
        }
    }

    #[test]
    fn parse_source_handles_part_suffixes_and_parenthesised_urls() {
        assert_eq!(
            parse_source("[[실험] 쿠션](https://cafe.example/5736) (part 1/2)"),
            ParsedSource {
                title: "[실험] 쿠션".to_string(),
                url: Some("https://cafe.example/5736".to_string()),
                part: Some((1, 2)),
            }
        );
        assert_eq!(
            parse_source("[Images.dat](https://book.example/Drawing(Remap).html#x)"),
            ParsedSource {
                title: "Images.dat".to_string(),
                url: Some("https://book.example/Drawing(Remap).html#x".to_string()),
                part: None,
            }
        );
        let plain = parse_source("plain source (part 1/2)");
        assert_eq!(plain.url, None);
        assert_eq!(plain.title, "plain source (part 1/2)");
    }

    #[test]
    fn rag_search_payload_returns_parsed_hits_for_the_user() {
        let rag = Rag::new(
            vec![
                entry(
                    0x2a,
                    2,
                    "[채팅 강좌](https://cafe.example/42) (part 2/3)",
                    "chatEvent 는 채팅을 감지합니다",
                ),
                entry(7, 3, "[other](https://x.example/7)", "unrelated"),
            ],
            None,
        );

        let response = rag_search_payload(&rag, "  chatEvent ");
        assert_eq!(response.query, "chatEvent");
        assert_eq!(response.index_size, 2);
        assert!(!response.semantic_ready);
        assert_eq!(response.hits.len(), 1);
        let hit = &response.hits[0];
        assert_eq!(hit.id, "000000000000002a");
        assert_eq!(hit.title, "채팅 강좌");
        assert_eq!(hit.url.as_deref(), Some("https://cafe.example/42"));
        assert_eq!(hit.part, Some((2, 3)));
        assert_eq!(hit.tier, "lecture");
        assert_eq!(hit.match_kind, "lexical");
        assert_eq!(hit.text, "chatEvent 는 채팅을 감지합니다");

        assert!(rag_search_payload(&rag, "   ").hits.is_empty());
    }

    #[test]
    fn rag_article_joins_every_part_and_drops_the_overlaps() {
        let source = |part: u32| format!("[강좌](https://cafe.example/9) (part {part}/3)");
        let rag = Rag::new(
            vec![
                entry(3, 2, &source(3), "세 번째 조각의 겹침 문장입니다. 끝."),
                entry(
                    1,
                    2,
                    &source(1),
                    "첫 문단.\n두 번째 조각과 겹치는 문장입니다.",
                ),
                entry(
                    2,
                    2,
                    &source(2),
                    "두 번째 조각과 겹치는 문장입니다. 이어서 세 번째 조각의 겹침 문장입니다.",
                ),
                entry(4, 1, "[단일](https://cafe.example/10)", "단일 본문"),
            ],
            None,
        );

        let article = rag_article_payload(&rag, 2).unwrap();
        assert_eq!(article.title, "강좌");
        assert_eq!(article.parts, 3);
        assert!(article.complete);
        assert_eq!(
            article.text,
            "첫 문단.\n두 번째 조각과 겹치는 문장입니다. 이어서 세 번째 조각의 겹침 문장입니다. 끝."
        );

        let single = rag_article_payload(&rag, 4).unwrap();
        assert_eq!((single.parts, single.complete), (1, true));
        assert_eq!(single.text, "단일 본문");
        assert!(rag_article_payload(&rag, 99).is_none());
    }

    #[test]
    fn rag_article_falls_back_to_the_chunk_when_parts_are_ambiguous() {
        let source = |part: u32| format!("[공유 링크](https://cafe.example/1) (part {part}/2)");
        let rag = Rag::new(
            vec![
                entry(1, 0, &source(1), "첫 글 앞부분"),
                entry(2, 0, &source(1), "다른 글 앞부분"),
                entry(3, 0, &source(2), "뒷부분"),
            ],
            None,
        );

        let article = rag_article_payload(&rag, 2).unwrap();
        assert!(!article.complete);
        assert_eq!(article.parts, 1);
        assert_eq!(article.text, "다른 글 앞부분");
    }
}
