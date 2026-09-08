# RAG index builder

This standalone crate builds the CI RAG artifact from the checked-in `ci/corpus` JSONL files.
It does not depend on `eud-agent`, `eud_agent_lib`, sqlite, or the ECA chromadb stores.

Run:

```powershell
cargo run --release --manifest-path ci/Cargo.toml --bin build_rag_index -- --corpus ci/corpus --out rag-index.bin
```

The nine fixed inputs under `--corpus` (default `ci/corpus`) are:

- `articles.jsonl`
- `eud_book.jsonl`
- `cafebook.jsonl`
- `scrmapdocs_en.jsonl`
- `eudplib_api.jsonl`
- `eudplib_examples.jsonl`
- `eud_editor_schema.jsonl`
- `eudtools_wiki.jsonl`
- `eudtools_reference.jsonl`

Output:

- `rag-index.bin`, using the `ERAG` v2 little-endian layout loaded by `src-tauri/src/rag.rs`
- `rag-index.bin.sha256`, containing the lowercase SHA-256 hex digest of the `.bin`

The workflow generates `rag-index.manifest.json` from the actual binary digest. Release
generation `4` (`rag-index-v4`) is independent of binary layout version `2`; bumping the
release generation refreshes installed corpus assets without changing the index format.
Local generation does not publish a release.

Rows are split before embedding when the full document text exceeds `CHUNK_CHARS = 2000`.
Chunking is on UTF-8 character boundaries, not byte offsets, with `CHUNK_OVERLAP = 200`
characters between consecutive chunks. Each chunk is embedded and written as its own index
entry. Per-chunk ids are deterministic FNV-1a hashes of the stable row key plus `#<chunk_index>`;
multi-chunk sources append `(part n/total)` to the normal citation header.

The first real run needs network access once to download fastembed's bge-m3 `BGEM3Q` int8
model, about 570 MB. By default fastembed caches it under the current directory's
`.fastembed_cache/`; pass `--cache <dir>` to use a different cache directory.

The canonical build uses CPU embedding with `DEFAULT_BATCH_SIZE = 16`. The quantized model
is batch-size-sensitive; do not change the batch size or use GPU output as an interchangeable
artifact without the separate compatibility gate.

## Tiers and historical migration

Existing primary/reference sources retain tier 3. General eudtools technical material uses
tier 2; experimental wiki rows use tier 1 via their `source` label
`eudtools_wiki_experimental.jsonl`. The latter is stored inside `eudtools_wiki.jsonl`,
not read as a tenth input. Runtime ranking weights and chunk ID derivation are unchanged.

`migrate_rag_index` converts historical ERAG v1 assets to layout v2 without embedding.
It permits only the two new eudtools corpus files to be absent from a historical snapshot;
the original seven remain mandatory. Every present corpus chunk and input binary entry
must join exactly by ID. Unmatched rows or entries fail migration, and existing vector
bytes are copied unchanged. Adding eudtools content to an old index requires a full
canonical rebuild, not migration.
