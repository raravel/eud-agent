#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Generate the RAG embedding-parity fixture (EUD-107).

Produces a self-contained fixture comparing the Rust fastembed bge-m3 path against
the Python `sentence-transformers` BAAI/bge-m3 baseline. We re-embed a FIXED corpus
subset + query set on BOTH sides and compare top-5 rankings, so the test isolates the
EMBEDDING-SPACE parity (the actual question) from any index/store difference.

This script is the PYTHON (baseline) half: it reads a deterministic sample of the
read-only ECA corpus (NEVER modified), embeds corpus + queries with
SentenceTransformer("BAAI/bge-m3", normalize_embeddings=True) exactly as ECA's
rag_query.py --bge path does, computes per-query top-5 corpus indices by cosine
(== dot on normalized vectors), and writes the fixture JSON consumed by
`src-tauri/tests/rag_parity.rs`.

Run with the ECA venv (it already has sentence-transformers + the bge-m3 HF cache):
    C:/Users/ifthe/proj/eud/ECA/.venv/Scripts/python.exe \
        ci/gen_rag_parity_fixture.py

Output: src-tauri/tests/fixtures/rag_parity.json
"""
import json
import os
import sys

# Read-only ECA corpus (never modified). Absolute path kept out of the committed
# fixture; only this generator references it.
ECA_DIR = r"C:\Users\ifthe\proj\eud\ECA"
OUT = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "..", "src-tauri", "tests", "fixtures", "rag_parity.json",
)

MODEL = "BAAI/bge-m3"
# Per-doc char budget: keep Python and Rust embedding the IDENTICAL input and keep
# the spike fast. bge-m3 handles long inputs; a fixed budget makes the two sides
# byte-for-byte comparable.
CHAR_BUDGET = 1500

# Deterministic stride sample per source -> ~180 docs with a spread across the
# eud-book reference, the cafe book, and the board articles. No randomness (stable
# fixture across regenerations).
SOURCES = [
    ("eud_book.jsonl", 10),
    ("cafebook.jsonl", 2),
    ("articles.jsonl", 80),
]

# Fixed Korean EUD query set (community jargon — the corpus is Korean; English
# queries lose recall). These probe distinct topics so top-5 is discriminating.
QUERIES = [
    "음수 로케이션으로 피탄판정 하는 법",
    "MoveLocation 과 Bring 으로 유닛 위치 감지",
    "EUDLoopPlayer Human 플레이어 조건",
    "데스 카운트 트리거 설정",
    "버튼셋 disable 문자열 크래시",
    "epScript 함수 정의 문법",
    "스타크래프트 유닛 체력 주소 변경",
    "euddraft 빌드 오류 해결",
    "트리거 액션 텍스트 출력",
    "EPD 플레이어 변수 읽기",
]
TOP_K = 5


def load_corpus():
    docs = []
    for fname, stride in SOURCES:
        path = os.path.join(ECA_DIR, fname)
        with open(path, encoding="utf-8") as fh:
            for i, line in enumerate(fh):
                if i % stride != 0:
                    continue
                line = line.strip()
                if not line:
                    continue
                rec = json.loads(line)
                text = (rec.get("content") or "").strip()[:CHAR_BUDGET]
                if not text:
                    continue
                docs.append({
                    "text": text,
                    "title": rec.get("title", ""),
                    "source": rec.get("source", fname),
                })
    return docs


def main():
    docs = load_corpus()
    print(f"[parity] corpus subset: {len(docs)} docs, {len(QUERIES)} queries",
          file=sys.stderr)

    from sentence_transformers import SentenceTransformer
    import numpy as np

    model = SentenceTransformer(MODEL)
    corpus_emb = model.encode(
        [d["text"] for d in docs], normalize_embeddings=True,
        batch_size=16, show_progress_bar=True,
    )
    query_emb = model.encode(
        QUERIES, normalize_embeddings=True, batch_size=16,
    )

    corpus_emb = np.asarray(corpus_emb, dtype=np.float32)
    query_emb = np.asarray(query_emb, dtype=np.float32)

    baseline_top5 = []
    for qi in range(len(QUERIES)):
        # cosine == dot on L2-normalized vectors.
        scores = corpus_emb @ query_emb[qi]
        top = np.argsort(-scores)[:TOP_K].tolist()
        baseline_top5.append([int(x) for x in top])

    fixture = {
        "model": MODEL,
        "normalized": True,
        "dim": int(corpus_emb.shape[1]),
        "top_k": TOP_K,
        "char_budget": CHAR_BUDGET,
        # Indices in `corpus` are the shared id space for both sides.
        "corpus": [{"id": i, **d} for i, d in enumerate(docs)],
        "queries": QUERIES,
        "baseline_top5": baseline_top5,
    }

    os.makedirs(os.path.dirname(OUT), exist_ok=True)
    with open(OUT, "w", encoding="utf-8") as fh:
        json.dump(fixture, fh, ensure_ascii=False, indent=2)
    print(f"[parity] wrote {OUT} (dim={fixture['dim']})", file=sys.stderr)


if __name__ == "__main__":
    main()
