use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use fastembed::{Bgem3Embedding, Bgem3InitOptions, Bgem3Model};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const EMBED_DIM: usize = 1024;
const INDEX_MAGIC: &[u8; 4] = b"ERAG";
const INDEX_VERSION: u32 = 1;
const INPUT_FILES: [&str; 3] = ["articles.jsonl", "eud_book.jsonl", "cafebook.jsonl"];
const DEFAULT_ECA_DIR: &str = r"C:\Users\ifthe\proj\eud\ECA";
const EMBED_BATCH_SIZE: usize = 16;
const CHUNK_CHARS: usize = 2000;
const CHUNK_OVERLAP: usize = 200;

#[derive(Debug)]
struct Args {
    eca_dir: PathBuf,
    out: PathBuf,
    cache_dir: Option<PathBuf>,
}

#[derive(Debug)]
struct CorpusDoc {
    id: u64,
    text: String,
    source: String,
}

#[derive(Debug)]
struct IndexEntry {
    id: u64,
    vector: Vec<f32>,
    text: String,
    source: String,
}

#[derive(Debug, Deserialize)]
struct JsonlRow {
    id: Option<String>,
    title: String,
    #[serde(default)]
    url: Option<String>,
    source: String,
    content: String,
    #[serde(default)]
    comments: Option<String>,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let docs = read_corpus(&args.eca_dir)?;
    let entries = embed_docs(docs, args.cache_dir)?;
    write_index(&args.out, &entries)?;
    let digest = write_sha256_sidecar(&args.out)?;

    eprintln!(
        "rows written: {}\noutput: {}\nsha256: {}",
        entries.len(),
        args.out.display(),
        digest
    );

    Ok(())
}

fn parse_args() -> Result<Args> {
    let mut eca_dir = env::var_os("ECA_DIR").map(PathBuf::from);
    let mut out = None;
    let mut cache_dir = None;

    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--eca" => {
                eca_dir = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--eca requires a directory path"))?,
                ));
            }
            "--out" => {
                out =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        anyhow!("--out requires an output file path")
                    })?));
            }
            "--cache" => {
                cache_dir =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        anyhow!("--cache requires a cache directory path")
                    })?));
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => bail!("unknown argument {other:?}; run with --help for usage"),
        }
    }

    Ok(Args {
        eca_dir: eca_dir.unwrap_or_else(|| PathBuf::from(DEFAULT_ECA_DIR)),
        out: out.unwrap_or_else(|| PathBuf::from("rag-index.bin")),
        cache_dir,
    })
}

fn print_usage() {
    eprintln!(
        "usage: build_rag_index [--eca <dir>] [--out <file>] [--cache <dir>]\n\
         defaults: --eca %ECA_DIR% or {DEFAULT_ECA_DIR}; --out rag-index.bin"
    );
}

fn read_corpus(eca_dir: &Path) -> Result<Vec<CorpusDoc>> {
    let mut docs = Vec::new();

    for file_name in INPUT_FILES {
        let path = eca_dir.join(file_name);
        let file =
            File::open(&path).with_context(|| format!("open JSONL input {}", path.display()))?;
        let reader = BufReader::new(file);

        for (zero_based_line, line) in reader.lines().enumerate() {
            let line_number = zero_based_line + 1;
            let line =
                line.with_context(|| format!("read {} line {line_number}", path.display()))?;
            if line.trim().is_empty() {
                continue;
            }

            let row: JsonlRow = serde_json::from_str(&line)
                .with_context(|| format!("parse {} line {line_number}", path.display()))?;
            docs.extend(corpus_docs_from_row(row, file_name, line_number));
        }
    }

    Ok(docs)
}

fn corpus_docs_from_row(row: JsonlRow, file_name: &str, line_number: usize) -> Vec<CorpusDoc> {
    let content = row.content.trim();
    let comments = row.comments.as_deref().unwrap_or("").trim();
    if content.is_empty() && comments.is_empty() {
        return Vec::new();
    }

    let title = row.title.trim();
    let url = row.url.as_deref().unwrap_or("").trim();
    let mut text = format!("제목: {title}\n\n{content}");
    if !comments.is_empty() {
        text.push_str("\n\n[댓글]\n");
        text.push_str(comments);
    }

    let source = if url.is_empty() {
        format!("[{title}]")
    } else {
        format!("[{title}]({url})")
    };

    let key = row
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| format!("id:{id}"))
        .or_else(|| (!url.is_empty()).then(|| format!("url:{url}")))
        .unwrap_or_else(|| format!("source:{}:{file_name}:{line_number}", row.source));

    let chunks = chunk_text(text);
    let total_chunks = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(chunk_index, chunk_text)| {
            let chunk_key = format!("{key}#{chunk_index}");
            let chunk_source = if total_chunks == 1 {
                source.clone()
            } else {
                format!("{source} (part {}/{total_chunks})", chunk_index + 1)
            };

            CorpusDoc {
                // Stable ids are FNV-1a 64-bit hashes of a deterministic key:
                // input id if present, else URL, else source + file + 1-based
                // line number, plus #<chunk_index> so chunks stay unique.
                id: fnv1a64(chunk_key.as_bytes()),
                text: chunk_text,
                source: chunk_source,
            }
        })
        .collect()
}

fn chunk_text(text: String) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= CHUNK_CHARS {
        return vec![text];
    }

    let step = CHUNK_CHARS - CHUNK_OVERLAP;
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < chars.len() {
        let end = (start + CHUNK_CHARS).min(chars.len());
        chunks.push(chars[start..end].iter().collect());
        if end == chars.len() {
            break;
        }
        start += step;
    }

    chunks
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn embed_docs(docs: Vec<CorpusDoc>, cache_dir: Option<PathBuf>) -> Result<Vec<IndexEntry>> {
    let mut opts = Bgem3InitOptions::new(Bgem3Model::BGEM3Q);
    if let Some(dir) = cache_dir {
        opts = opts.with_cache_dir(dir);
    }

    let mut embedder =
        Bgem3Embedding::try_new(opts).map_err(|e| anyhow!("model init failed: {e}"))?;
    let mut entries = Vec::with_capacity(docs.len());

    for chunk in docs.chunks(EMBED_BATCH_SIZE) {
        let texts: Vec<String> = chunk.iter().map(|doc| doc.text.clone()).collect();
        let output = embedder
            .embed(&texts, None)
            .map_err(|e| anyhow!("embedding batch failed: {e}"))?;

        if output.dense.len() != chunk.len() {
            bail!(
                "embedding batch returned {} dense vectors for {} inputs",
                output.dense.len(),
                chunk.len()
            );
        }

        for (doc, mut vector) in chunk.iter().zip(output.dense) {
            if vector.len() != EMBED_DIM {
                bail!(
                    "doc id {} produced {}-d vector, expected {EMBED_DIM}-d",
                    doc.id,
                    vector.len()
                );
            }
            l2_normalize(&mut vector);
            entries.push(IndexEntry {
                id: doc.id,
                vector,
                text: doc.text.clone(),
                source: doc.source.clone(),
            });
        }
    }

    Ok(entries)
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

fn write_index(path: &Path, entries: &[IndexEntry]) -> Result<()> {
    if entries.len() > u32::MAX as usize {
        bail!(
            "index has {} entries; v1 format count is u32",
            entries.len()
        );
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
    }

    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut w = BufWriter::new(file);

    w.write_all(INDEX_MAGIC)?;
    w.write_all(&INDEX_VERSION.to_le_bytes())?;
    w.write_all(&(entries.len() as u32).to_le_bytes())?;

    for entry in entries {
        if entry.vector.len() != EMBED_DIM {
            bail!(
                "entry id {} has {}-d vector, expected {EMBED_DIM}-d",
                entry.id,
                entry.vector.len()
            );
        }

        w.write_all(&entry.id.to_le_bytes())?;
        for value in &entry.vector {
            w.write_all(&value.to_le_bytes())?;
        }
        write_len_prefixed(&mut w, entry.text.as_bytes(), "text")?;
        write_len_prefixed(&mut w, entry.source.as_bytes(), "source")?;
    }

    w.flush()?;
    Ok(())
}

fn write_len_prefixed(w: &mut BufWriter<File>, bytes: &[u8], field: &str) -> Result<()> {
    if bytes.len() > u32::MAX as usize {
        bail!("{field} is {} bytes; v1 format length is u32", bytes.len());
    }
    w.write_all(&(bytes.len() as u32).to_le_bytes())?;
    w.write_all(bytes)?;
    Ok(())
}

fn write_sha256_sidecar(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let digest = Sha256::digest(&bytes);
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    let sidecar = sha256_sidecar_path(path);
    fs::write(&sidecar, format!("{hex}\n"))
        .with_context(|| format!("write {}", sidecar.display()))?;
    Ok(hex)
}

fn sha256_sidecar_path(path: &Path) -> PathBuf {
    let mut out = OsString::from(path.as_os_str());
    out.push(".sha256");
    PathBuf::from(out)
}
