# RAG index builder

This standalone crate builds the CI RAG artifact from the read-only ECA corpus JSONL files.
It does not depend on `eud-agent`, `eud_agent_lib`, sqlite, or the ECA chromadb stores.

Run:

```powershell
cargo run --manifest-path ci/Cargo.toml --bin build_rag_index -- --eca C:\Users\ifthe\proj\eud\ECA --out rag-index.bin
```

Inputs under `--eca`:

- `articles.jsonl`
- `eud_book.jsonl`
- `cafebook.jsonl`

Output:

- `rag-index.bin`, using the `ERAG` v1 little-endian layout loaded by `src-tauri/src/rag.rs`
- `rag-index.bin.sha256`, containing the lowercase SHA-256 hex digest of the `.bin`

Rows are split before embedding when the full document text exceeds `CHUNK_CHARS = 2000`.
Chunking is on UTF-8 character boundaries, not byte offsets, with `CHUNK_OVERLAP = 200`
characters between consecutive chunks. Each chunk is embedded and written as its own index
entry. Per-chunk ids are deterministic FNV-1a hashes of the stable row key plus `#<chunk_index>`;
multi-chunk sources append `(part n/total)` to the normal citation header.

The first real run needs network access once to download fastembed's bge-m3 `BGEM3Q` int8
model, about 570 MB. By default fastembed caches it under the current directory's
`.fastembed_cache/`; pass `--cache <dir>` to use a different cache directory.
