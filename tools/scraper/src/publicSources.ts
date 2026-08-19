import { execFile } from "node:child_process";
import { mkdtemp, readdir, readFile, rename, rm, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join, relative, sep } from "node:path";
import { corpusOutputDir } from "./config.js";
import {
  type CorpusJsonRow,
  writeCorpusJsonlAtomic
} from "./corpusWriter.js";

const SCR_SOURCE = "scrmapdocs_en.jsonl";
const EUDPLIB_API_SOURCE = "eudplib_api.jsonl";
const EUDPLIB_EXAMPLE_SOURCE = "eudplib_examples.jsonl";
const EDITOR_SOURCE = "eud_editor_schema.jsonl";
const EUD_BOOK_SOURCE = "eud_book.jsonl";

const repositorySpecs = {
  scrmapdocs: {
    slug: "havonz/SCRMapDocs",
    url: "https://github.com/havonz/SCRMapDocs.git",
    sparsePaths: ["docs"]
  },
  eudplib: {
    slug: "armoha/eudplib",
    url: "https://github.com/armoha/eudplib.git",
    sparsePaths: ["docs", "src/eudplib", "tests"]
  },
  eudBook: {
    slug: "armoha/eud-book",
    url: "https://github.com/armoha/eud-book.git",
    sparsePaths: ["api.json", "docs/searchindex.json"]
  },
  editor: {
    slug: "Buizz/EUD-Editor-3",
    url: "https://github.com/Buizz/EUD-Editor-3.git",
    sparsePaths: [
      "EUD Editor 3/AvalonEdit/CodeEditor",
      "EUD Editor 3/Class/BulidData",
      "EUD Editor 3/Class/Data",
      "EUD Editor 3/Class/ExtraData",
      "EUD Editor 3/Class/TriggerEditor",
      "EUD Editor 3/Data/DatFiles",
      "EUD Editor 3/Data/TriggerEditor/epsFunctions_safe.txt",
      "EUD Editor 3/Module/Tools",
      "EUD Editor 3/Version.txt"
    ]
  }
} as const;

const editorContractPaths = [
  "EUD Editor 3/AvalonEdit/CodeEditor/TriggerEditorCompletionData.vb",
  "EUD Editor 3/Class/BulidData/WriteButtonData.vb",
  "EUD Editor 3/Class/BulidData/WriteDatFile.vb",
  "EUD Editor 3/Class/BulidData/WriteReqFile.vb",
  "EUD Editor 3/Class/BulidData/WriteTriggerEditor.vb",
  "EUD Editor 3/Class/BulidData/WriteedsFile.vb",
  "EUD Editor 3/Class/Data/CButtonData.vb",
  "EUD Editor 3/Class/Data/CRequireData.vb",
  "EUD Editor 3/Class/Data/ProgramData.vb",
  "EUD Editor 3/Class/Data/ProjectData/ProjectData.vb",
  "EUD Editor 3/Class/Data/SCDatFiles.vb",
  "EUD Editor 3/Class/ExtraData/ExtraDatFiles.vb",
  "EUD Editor 3/Class/TriggerEditor/TEFile.vb",
  "EUD Editor 3/Class/TriggerEditor/TriggerEditorData.vb",
  "EUD Editor 3/Module/Tools/BuildErrorHandling.vb"
] as const;

export type PublicSyncSummary = {
  outputFile: string;
  rows: number;
  commit: string;
};

type RepositorySpec = {
  slug: string;
  url: string;
  sparsePaths: readonly string[];
};

type RepositorySnapshot = {
  root: string;
  slug: string;
  commit: string;
};

type MarkdownSection = {
  key: string;
  title: string;
  content: string;
};

type PythonDefinition = {
  name: string;
  kind: "class" | "function";
  signature: string;
  documentation?: string;
  methods: string[];
};

type EudBookSearchIndex = {
  doc_urls: string[];
  index: {
    documentStore: {
      docs: Record<
        string,
        {
          body?: string;
          breadcrumbs?: string;
          title?: string;
        }
      >;
    };
  };
};

export async function syncPublicSources(
  outputDir = corpusOutputDir
): Promise<PublicSyncSummary[]> {
  const tempRoot = await mkdtemp(join(tmpdir(), "eud-agent-upstream-"));

  try {
    const scr = await cloneSnapshot(repositorySpecs.scrmapdocs, tempRoot, "scrmapdocs");
    const eudplib = await cloneSnapshot(repositorySpecs.eudplib, tempRoot, "eudplib");
    const eudBook = await cloneSnapshot(repositorySpecs.eudBook, tempRoot, "eud-book");
    const editor = await cloneSnapshot(repositorySpecs.editor, tempRoot, "editor3");

    const scrRows = await buildScrMapDocsRows(scr);
    const { apiRows, exampleRows } = await buildEudplibRows(eudplib);
    const eudBookRows = await buildEudBookRows(eudBook);
    const editorRows = await buildEditorRows(editor);

    const outputs: Array<[string, CorpusJsonRow[], string]> = [
      [SCR_SOURCE, scrRows, scr.commit],
      [EUDPLIB_API_SOURCE, apiRows, eudplib.commit],
      [EUDPLIB_EXAMPLE_SOURCE, exampleRows, eudplib.commit],
      [EUD_BOOK_SOURCE, eudBookRows, eudBook.commit],
      [EDITOR_SOURCE, editorRows, editor.commit]
    ];

    for (const [fileName, rows] of outputs) {
      await writeCorpusJsonlAtomic(join(outputDir, fileName), rows);
    }

    await writeThirdPartyNotices(outputDir, [scr, eudplib, editor]);

    return outputs.map(([outputFile, rows, commit]) => ({
      outputFile,
      rows: rows.length,
      commit
    }));
  } finally {
    await rm(tempRoot, { recursive: true, force: true });
  }
}

async function cloneSnapshot(
  spec: RepositorySpec,
  tempRoot: string,
  folder: string
): Promise<RepositorySnapshot> {
  const root = join(tempRoot, folder);
  await runGit([
    "clone",
    "--depth",
    "1",
    "--filter=blob:none",
    "--sparse",
    spec.url,
    root
  ]);
  await runGit([
    "-C",
    root,
    "sparse-checkout",
    "set",
    "--skip-checks",
    ...spec.sparsePaths
  ]);
  const commit = (await runGit(["-C", root, "rev-parse", "HEAD"])).trim();

  if (!/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error(`Unexpected commit returned for ${spec.slug}: ${commit}`);
  }

  return { root, slug: spec.slug, commit };
}

async function runGit(args: string[]): Promise<string> {
  const { promise, resolve, reject } = Promise.withResolvers<string>();
  execFile(
    "git",
    args,
    {
      encoding: "utf8",
      maxBuffer: 16 * 1024 * 1024,
      windowsHide: true
    },
    (error, stdout, stderr) => {
      if (error) {
        reject(
          new Error(
            `git ${args.join(" ")} failed: ${String(stderr).trim() || error.message}`
          )
        );
        return;
      }
      resolve(String(stdout));
    }
  );
  return promise;
}

async function buildScrMapDocsRows(
  snapshot: RepositorySnapshot
): Promise<CorpusJsonRow[]> {
  const docsRoot = join(snapshot.root, "docs");
  const paths = await listFiles(docsRoot, (path) => path.endsWith(".md"));
  const rows: CorpusJsonRow[] = [];

  for (const path of paths) {
    const repoPath = toRepoPath(snapshot.root, path);
    const markdown = stripBom(await readFile(path, "utf8"));
    rows.push(
      ...markdownRows({
        markdown,
        repoPath,
        snapshot,
        source: SCR_SOURCE,
        titlePrefix: "SCRMapDocs",
        scope: "epScript and standalone euddraft reference",
        language: "English"
      })
    );
  }

  return sortRows(rows);
}

async function buildEudplibRows(snapshot: RepositorySnapshot): Promise<{
  apiRows: CorpusJsonRow[];
  exampleRows: CorpusJsonRow[];
}> {
  const packageRoot = join(snapshot.root, "src", "eudplib");
  const initPath = join(packageRoot, "__init__.py");
  const initText = stripBom(await readFile(initPath, "utf8"));
  const version = initText.match(/__version__\s*=\s*["']([^"']+)["']/)?.[1] ?? "unknown";
  const pythonPaths = await listFiles(packageRoot, (path) => path.endsWith(".py"));
  const exports = new Set<string>(["eudplibVersion"]);

  for (const path of pythonPaths.filter((path) => basename(path) === "__init__.py")) {
    const text = stripBom(await readFile(path, "utf8"));
    for (const name of extractPythonExports(text)) {
      exports.add(name);
    }
  }

  const apiRows: CorpusJsonRow[] = [
    makeRow({
      id: `${snapshot.slug}:exports`,
      title: `[eudplib ${version}] Public API export catalog`,
      content: [
        `Snapshot commit: ${snapshot.commit}`,
        "Scope: Python eudplib public API. Do not paste Python syntax into an epScript file.",
        "Exported symbols:",
        [...exports].sort().join(", ")
      ].join("\n\n"),
      url: githubBlobUrl(snapshot, "src/eudplib/__init__.py"),
      source: EUDPLIB_API_SOURCE,
      snapshot,
      repoPath: "src/eudplib/__init__.py",
      version,
      language: "Python",
      scope: "eudplib public API"
    })
  ];

  for (const path of pythonPaths) {
    const text = stripBom(await readFile(path, "utf8"));
    const repoPath = toRepoPath(snapshot.root, path);
    for (const definition of extractPythonDefinitions(text)) {
      if (!exports.has(definition.name) || definition.name.startsWith("_")) {
        continue;
      }

      const details = [
        `Snapshot commit: ${snapshot.commit}`,
        `eudplib version: ${version}`,
        "Scope: Python eudplib public API. Do not paste Python syntax into an epScript file.",
        `${definition.kind === "class" ? "Class" : "Function"} signature:`,
        definition.signature
      ];
      if (definition.documentation) {
        details.push("Documentation:", definition.documentation);
      }
      if (definition.methods.length > 0) {
        details.push("Public method signatures:", definition.methods.join("\n"));
      }

      apiRows.push(
        makeRow({
          id: `${snapshot.slug}:${repoPath}#${definition.name}`,
          title: `[eudplib ${version}] ${definition.name}`,
          content: details.join("\n\n"),
          url: githubBlobUrl(snapshot, repoPath),
          source: EUDPLIB_API_SOURCE,
          snapshot,
          repoPath,
          version,
          language: "Python",
          scope: "eudplib public API"
        })
      );
    }
  }

  const docsRoot = join(snapshot.root, "docs");
  const docPaths = await listFiles(docsRoot, (path) => path.endsWith(".md"));
  for (const path of docPaths) {
    const repoPath = toRepoPath(snapshot.root, path);
    apiRows.push(
      ...markdownRows({
        markdown: stripBom(await readFile(path, "utf8")),
        repoPath,
        snapshot,
        source: EUDPLIB_API_SOURCE,
        titlePrefix: `eudplib ${version}`,
        scope: "Python eudplib maintained documentation",
        language: "Korean/English",
        version
      })
    );
  }

  const testsRoot = join(snapshot.root, "tests");
  const epsPaths = await listFiles(testsRoot, (path) => path.endsWith(".eps"));
  const exampleRows: CorpusJsonRow[] = [];
  for (const path of epsPaths) {
    const repoPath = toRepoPath(snapshot.root, path);
    const chunks = chunkByLines(stripBom(await readFile(path, "utf8")), 1500);
    chunks.forEach((chunk, index) => {
      exampleRows.push(
        makeRow({
          id: `${snapshot.slug}:${repoPath}#${index}`,
          title: `[eudplib ${version} epScript test] ${basename(path)} part ${index + 1}/${chunks.length}`,
          content: [
            `Snapshot commit: ${snapshot.commit}`,
            "Scope: official epScript compiler test/example.",
            chunk
          ].join("\n\n"),
          url: githubBlobUrl(snapshot, repoPath),
          source: EUDPLIB_EXAMPLE_SOURCE,
          snapshot,
          repoPath,
          version,
          language: "epScript",
          scope: "official compiler test/example"
        })
      );
    });
  }

  return {
    apiRows: sortRows(apiRows),
    exampleRows: sortRows(exampleRows)
  };
}

async function buildEudBookRows(
  snapshot: RepositorySnapshot
): Promise<CorpusJsonRow[]> {
  const repoPath = "docs/searchindex.json";
  const parsed = JSON.parse(
    stripBom(await readFile(join(snapshot.root, ...repoPath.split("/")), "utf8"))
  ) as EudBookSearchIndex;
  const docs = parsed.index.documentStore.docs;
  const rows: CorpusJsonRow[] = [];

  for (const id of Object.keys(docs).sort(compareNumericStrings)) {
    const doc = docs[id];
    const title = doc.title?.trim() ?? "";
    const body = doc.body?.trim() ?? "";
    if (!title || !body || id === "0") {
      continue;
    }

    const docUrl = parsed.doc_urls[Number.parseInt(id, 10)];
    if (!docUrl) {
      throw new Error(`eud-book search index is missing doc_urls[${id}]`);
    }

    rows.push(
      makeRow({
        id: `${snapshot.slug}:${id}`,
        title: `[eud-book] ${title}`,
        content: [
          `Snapshot commit: ${snapshot.commit}`,
          "Scope: StarCraft memory/offset reference.",
          doc.breadcrumbs?.trim(),
          body
        ]
          .filter((part): part is string => Boolean(part))
          .join("\n\n"),
        url: `https://armoha.github.io/eud-book/${docUrl}`,
        source: EUD_BOOK_SOURCE,
        snapshot,
        repoPath,
        language: "English",
        scope: "StarCraft memory/offset reference"
      })
    );
  }

  return sortRows(rows);
}

async function buildEditorRows(
  snapshot: RepositorySnapshot
): Promise<CorpusJsonRow[]> {
  const editorRoot = join(snapshot.root, "EUD Editor 3");
  const versionText = stripBom(await readFile(join(editorRoot, "Version.txt"), "utf8"));
  const version = versionText.split(/\r?\n/, 1)[0]?.trim() || "unknown";
  const rows: CorpusJsonRow[] = [];
  const datRoot = join(editorRoot, "Data", "DatFiles");
  const definitionPaths = await listFiles(datRoot, (path) => path.endsWith(".def"));

  for (const path of definitionPaths) {
    const repoPath = toRepoPath(snapshot.root, path);
    rows.push(
      ...parseDatDefinitionRows({
        text: stripBom(await readFile(path, "utf8")),
        datName: basename(path, ".def"),
        repoPath,
        snapshot,
        version
      })
    );
  }

  const autocompletePath =
    "EUD Editor 3/Data/TriggerEditor/epsFunctions_safe.txt";
  rows.push(
    ...parseEditorFunctionRows({
      text: stripBom(
        await readFile(join(snapshot.root, ...autocompletePath.split("/")), "utf8")
      ),
      repoPath: autocompletePath,
      snapshot,
      version
    })
  );

  for (const repoPath of editorContractPaths) {
    const path = join(snapshot.root, ...repoPath.split("/"));
    const text = stripBom(await readFile(path, "utf8"));
    const chunks = chunkByParagraphs(text, 1500);
    chunks.forEach((chunk, index) => {
      rows.push(
        makeRow({
          id: `${snapshot.slug}:${repoPath}#${index}`,
          title: `[EUD Editor ${version} source contract] ${basename(repoPath)} part ${index + 1}/${chunks.length}`,
          content: [
            `Snapshot commit: ${snapshot.commit}`,
            "Scope: EUD Editor 3 internal model/build contract. This is not epScript syntax.",
            chunk
          ].join("\n\n"),
          url: githubBlobUrl(snapshot, repoPath),
          source: EDITOR_SOURCE,
          snapshot,
          repoPath,
          version,
          language: "VB.NET",
          scope: "editor internal model/build contract"
        })
      );
    });
  }

  return sortRows(rows);
}

export function splitMarkdownSections(markdown: string): MarkdownSection[] {
  const lines = stripBom(markdown).replace(/\r\n?/g, "\n").split("\n");
  const sections: MarkdownSection[] = [];
  const headingStack: string[] = [];
  const seenKeys = new Map<string, number>();
  let buffer: string[] = [];
  let inFence = false;

  const flush = () => {
    const content = buffer.join("\n").trim();
    buffer = [];
    if (!hasMeaningfulMarkdown(content)) {
      return;
    }

    const title =
      headingStack.filter((heading) => heading.length > 0).join(" > ") || "Overview";
    const baseKey = slugify(title) || "overview";
    const occurrence = seenKeys.get(baseKey) ?? 0;
    seenKeys.set(baseKey, occurrence + 1);
    sections.push({
      key: occurrence === 0 ? baseKey : `${baseKey}-${occurrence + 1}`,
      title,
      content
    });
  };

  for (const line of lines) {
    if (/^\s*```/.test(line) || /^\s*~~~/.test(line)) {
      inFence = !inFence;
      buffer.push(line);
      continue;
    }

    const heading = inFence
      ? undefined
      : line.match(/^\s*(?:-\s+)?(#{1,6})\s+(.+?)\s*#*\s*$/);
    if (!heading) {
      buffer.push(line);
      continue;
    }

    flush();
    const level = heading[1].length;
    const title = cleanHeading(heading[2]);
    headingStack.length = Math.min(headingStack.length, level - 1);
    headingStack[level - 1] = title;
    buffer.push(line);
  }

  flush();
  return sections;
}

function markdownRows(options: {
  markdown: string;
  repoPath: string;
  snapshot: RepositorySnapshot;
  source: string;
  titlePrefix: string;
  scope: string;
  language: string;
  version?: string;
}): CorpusJsonRow[] {
  return splitMarkdownSections(options.markdown).map((section) =>
    makeRow({
      id: `${options.snapshot.slug}:${options.repoPath}#${section.key}`,
      title: `[${options.titlePrefix}] ${section.title}`,
      content: [
        `Snapshot commit: ${options.snapshot.commit}`,
        `Scope: ${options.scope}.`,
        `Source language: ${options.language}.`,
        section.content
      ].join("\n\n"),
      url: githubBlobUrl(options.snapshot, options.repoPath),
      source: options.source,
      snapshot: options.snapshot,
      repoPath: options.repoPath,
      version: options.version,
      language: options.language,
      scope: options.scope
    })
  );
}

export function extractPythonExports(text: string): string[] {
  const exports = new Set<string>();
  const assignmentPattern = /__all__\s*=\s*\[([\s\S]*?)\]/g;
  for (const assignment of text.matchAll(assignmentPattern)) {
    for (const literal of assignment[1].matchAll(/["']([A-Za-z_][A-Za-z0-9_]*)["']/g)) {
      exports.add(literal[1]);
    }
  }
  return [...exports].sort();
}

export function extractPythonDefinitions(text: string): PythonDefinition[] {
  const lines = stripBom(text).replace(/\r\n?/g, "\n").split("\n");
  const definitions: PythonDefinition[] = [];

  for (let index = 0; index < lines.length; index += 1) {
    const match = lines[index].match(/^(async\s+def|def|class)\s+([A-Za-z_][A-Za-z0-9_]*)/);
    if (!match) {
      continue;
    }

    const headerEnd = findPythonHeaderEnd(lines, index);
    const signature = lines
      .slice(index, headerEnd + 1)
      .map((line) => line.trim())
      .join(" ");
    const blockEnd = findPythonBlockEnd(lines, headerEnd + 1);
    const documentation = extractPythonDocstring(lines, headerEnd + 1, blockEnd);
    const methods = match[1] === "class" ? extractClassMethods(lines, headerEnd + 1, blockEnd) : [];

    definitions.push({
      name: match[2],
      kind: match[1] === "class" ? "class" : "function",
      signature,
      documentation,
      methods
    });
    index = blockEnd - 1;
  }

  return definitions;
}

function findPythonHeaderEnd(lines: string[], start: number): number {
  for (let index = start; index < lines.length; index += 1) {
    if (lines[index].trimEnd().endsWith(":")) {
      return index;
    }
  }
  return start;
}

function findPythonBlockEnd(lines: string[], start: number): number {
  for (let index = start; index < lines.length; index += 1) {
    const line = lines[index];
    if (line.trim().length > 0 && !/^\s/.test(line)) {
      return index;
    }
  }
  return lines.length;
}

function extractPythonDocstring(
  lines: string[],
  start: number,
  end: number
): string | undefined {
  let index = start;
  while (index < end && (lines[index].trim() === "" || lines[index].trimStart().startsWith("#"))) {
    index += 1;
  }
  if (index >= end) {
    return undefined;
  }

  const trimmed = lines[index].trim();
  const match = trimmed.match(/^[rRuUbBfF]*("""|''')/);
  if (!match) {
    return undefined;
  }

  const quote = match[1];
  const collected: string[] = [];
  let remainder = trimmed.slice(match[0].length);
  const sameLineEnd = remainder.indexOf(quote);
  if (sameLineEnd >= 0) {
    return remainder.slice(0, sameLineEnd).trim() || undefined;
  }
  if (remainder.length > 0) {
    collected.push(remainder);
  }

  for (index += 1; index < end; index += 1) {
    const line = lines[index].trim();
    const closing = line.indexOf(quote);
    if (closing >= 0) {
      if (closing > 0) {
        collected.push(line.slice(0, closing));
      }
      break;
    }
    collected.push(line);
  }

  const documentation = collected.join("\n").trim();
  return documentation || undefined;
}

function extractClassMethods(lines: string[], start: number, end: number): string[] {
  const candidates: Array<{ indent: number; index: number }> = [];
  for (let index = start; index < end; index += 1) {
    const match = lines[index].match(/^(\s+)(?:async\s+def|def)\s+([A-Za-z_][A-Za-z0-9_]*)/);
    if (match && !match[2].startsWith("_")) {
      candidates.push({ indent: match[1].length, index });
    }
  }
  if (candidates.length === 0) {
    return [];
  }

  const methodIndent = Math.min(...candidates.map((candidate) => candidate.indent));
  return candidates
    .filter((candidate) => candidate.indent === methodIndent)
    .map((candidate) => {
      const headerEnd = findPythonHeaderEnd(lines, candidate.index);
      return lines
        .slice(candidate.index, headerEnd + 1)
        .map((line) => line.trim())
        .join(" ");
    });
}

export function parseDatDefinition(text: string): Array<Record<string, string>> {
  const parameters = new Map<string, Record<string, string>>();
  let inFormat = false;

  for (const rawLine of stripBom(text).replace(/\r\n?/g, "\n").split("\n")) {
    const line = rawLine.trim();
    if (line === "[FORMAT]") {
      inFormat = true;
      continue;
    }
    if (!inFormat || line.length === 0 || line.startsWith("[")) {
      continue;
    }

    const match = line.match(/^(\d+)([A-Za-z][A-Za-z0-9]*)=(.*)$/);
    if (!match) {
      continue;
    }
    const record = parameters.get(match[1]) ?? { index: match[1] };
    record[match[2]] = match[3].trim();
    parameters.set(match[1], record);
  }

  return [...parameters.values()].sort(
    (left, right) => Number.parseInt(left.index, 10) - Number.parseInt(right.index, 10)
  );
}

function parseDatDefinitionRows(options: {
  text: string;
  datName: string;
  repoPath: string;
  snapshot: RepositorySnapshot;
  version: string;
}): CorpusJsonRow[] {
  return parseDatDefinition(options.text).map((parameter) => {
    const name = parameter.Name || `parameter ${parameter.index}`;
    const details = Object.entries(parameter)
      .filter(([key]) => key !== "Name")
      .map(([key, value]) => `${key}: ${value}`)
      .join("\n");
    return makeRow({
      id: `${options.snapshot.slug}:${options.repoPath}#${parameter.index}`,
      title: `[EUD Editor ${options.version}/${options.datName}.dat] ${name}`,
      content: [
        `Snapshot commit: ${options.snapshot.commit}`,
        "Scope: exact EUD Editor 3 DAT parameter schema. Use the parameter name verbatim with dat_get/dat_set.",
        `DAT table: ${options.datName}`,
        `Parameter name: ${name}`,
        details
      ].join("\n\n"),
      url: githubBlobUrl(options.snapshot, options.repoPath),
      source: EDITOR_SOURCE,
      snapshot: options.snapshot,
      repoPath: options.repoPath,
      version: options.version,
      language: "EUD Editor DAT definition",
      scope: "editor DAT schema"
    });
  });
}

export function parseEditorFunctions(text: string): Array<{
  name: string;
  documentation: string;
  signature: string;
}> {
  const functions: Array<{
    name: string;
    documentation: string;
    signature: string;
  }> = [];
  const pattern = /\/\*\*\*([\s\S]*?)\*\/\s*(function\s+([A-Za-z_][A-Za-z0-9_]*)[^\n{]*\{\})/g;

  for (const match of stripBom(text).matchAll(pattern)) {
    functions.push({
      name: match[3],
      documentation: match[1]
        .split(/\r?\n/)
        .map((line) => line.replace(/^\s*\*+\s?/, ""))
        .join("\n")
        .trim(),
      signature: match[2].trim()
    });
  }

  return functions;
}

function parseEditorFunctionRows(options: {
  text: string;
  repoPath: string;
  snapshot: RepositorySnapshot;
  version: string;
}): CorpusJsonRow[] {
  return parseEditorFunctions(options.text).map((entry) =>
    makeRow({
      id: `${options.snapshot.slug}:${options.repoPath}#${entry.name}`,
      title: `[EUD Editor ${options.version} epScript API] ${entry.name}`,
      content: [
        `Snapshot commit: ${options.snapshot.commit}`,
        "Scope: epScript function advertised by the current EUD Editor 3 autocomplete data.",
        entry.signature,
        entry.documentation
      ].join("\n\n"),
      url: githubBlobUrl(options.snapshot, options.repoPath),
      source: EDITOR_SOURCE,
      snapshot: options.snapshot,
      repoPath: options.repoPath,
      version: options.version,
      language: "epScript",
      scope: "editor-provided epScript API"
    })
  );
}

function makeRow(options: {
  id: string;
  title: string;
  content: string;
  url: string;
  source: string;
  snapshot: RepositorySnapshot;
  repoPath: string;
  version?: string;
  language: string;
  scope: string;
}): CorpusJsonRow {
  return {
    id: options.id,
    title: options.title,
    content: options.content.trim(),
    url: options.url,
    source: options.source,
    commit: options.snapshot.commit,
    language: options.language,
    path: options.repoPath,
    repo: options.snapshot.slug,
    scope: options.scope,
    ...(options.version ? { version: options.version } : {})
  };
}

async function listFiles(
  root: string,
  predicate: (path: string) => boolean
): Promise<string[]> {
  const files: string[] = [];
  const walk = async (directory: string): Promise<void> => {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => left.name.localeCompare(right.name, "en"));
    for (const entry of entries) {
      const path = join(directory, entry.name);
      if (entry.isDirectory()) {
        await walk(path);
      } else if (entry.isFile() && predicate(path)) {
        files.push(path);
      }
    }
  };
  await walk(root);
  return files;
}

function toRepoPath(root: string, path: string): string {
  return relative(root, path).split(sep).join("/");
}

function githubBlobUrl(snapshot: RepositorySnapshot, repoPath: string): string {
  const encodedPath = repoPath
    .split("/")
    .map((segment) => encodeURIComponent(segment))
    .join("/");
  return `https://github.com/${snapshot.slug}/blob/${snapshot.commit}/${encodedPath}`;
}

function sortRows(rows: CorpusJsonRow[]): CorpusJsonRow[] {
  return [...rows].sort((left, right) =>
    String(left.id ?? left.url ?? left.title).localeCompare(
      String(right.id ?? right.url ?? right.title),
      "en"
    )
  );
}

function compareNumericStrings(left: string, right: string): number {
  return Number.parseInt(left, 10) - Number.parseInt(right, 10);
}

function stripBom(text: string): string {
  return text.replace(/^\uFEFF/, "");
}

function cleanHeading(value: string): string {
  return value
    .replace(/\[([^\]]+)\]\([^\)]+\)/g, "$1")
    .replace(/[\*`_]/g, "")
    .replace(/<[^>]+>/g, "")
    .trim();
}

function slugify(value: string): string {
  return value
    .normalize("NFKC")
    .toLowerCase()
    .replace(/[^\p{L}\p{N}]+/gu, "-")
    .replace(/^-+|-+$/g, "");
}

function hasMeaningfulMarkdown(value: string): boolean {
  return value
    .replace(/<br\s*\/?\s*>/gi, "")
    .replace(/<!--([\s\S]*?)-->/g, "")
    .trim().length > 0;
}

function chunkByLines(text: string, maxChars: number): string[] {
  const chunks: string[] = [];
  let current: string[] = [];
  let length = 0;

  for (const line of text.replace(/\r\n?/g, "\n").split("\n")) {
    const addition = line.length + (current.length > 0 ? 1 : 0);
    if (current.length > 0 && length + addition > maxChars) {
      chunks.push(current.join("\n").trim());
      current = [];
      length = 0;
    }
    current.push(line);
    length += addition;
  }
  if (current.length > 0) {
    chunks.push(current.join("\n").trim());
  }
  return chunks.filter((chunk) => chunk.length > 0);
}

function chunkByParagraphs(text: string, maxChars: number): string[] {
  const paragraphs = text.replace(/\r\n?/g, "\n").split(/\n{2,}/);
  const chunks: string[] = [];
  let current = "";

  for (const paragraph of paragraphs) {
    const clean = paragraph.trim();
    if (!clean) {
      continue;
    }
    if (clean.length > maxChars) {
      if (current) {
        chunks.push(current);
        current = "";
      }
      chunks.push(...chunkByLines(clean, maxChars));
      continue;
    }
    const candidate = current ? `${current}\n\n${clean}` : clean;
    if (candidate.length > maxChars) {
      chunks.push(current);
      current = clean;
    } else {
      current = candidate;
    }
  }
  if (current) {
    chunks.push(current);
  }
  return chunks;
}

async function writeThirdPartyNotices(
  outputDir: string,
  snapshots: RepositorySnapshot[]
): Promise<void> {
  const sections: string[] = [
    "Third-party source snapshots embedded in the RAG corpus.",
    "Generated by tools/scraper/src/publicSources.ts."
  ];

  for (const snapshot of snapshots) {
    const licensePath = join(snapshot.root, "LICENSE");
    const license = stripBom(await readFile(licensePath, "utf8"));
    sections.push(
      [
        "================================================================================",
        `${snapshot.slug} @ ${snapshot.commit}`,
        `https://github.com/${snapshot.slug}`,
        "--------------------------------------------------------------------------------",
        license.trim()
      ].join("\n")
    );
  }

  await writeTextAtomic(
    join(outputDir, "THIRD_PARTY_NOTICES.txt"),
    `${sections.join("\n\n")}\n`
  );
}

async function writeTextAtomic(path: string, content: string): Promise<void> {
  const tmpPath = `${path}.tmp`;
  await writeFile(tmpPath, content, "utf8");
  try {
    await rename(tmpPath, path);
  } catch (error) {
    await unlink(tmpPath).catch(() => undefined);
    throw error;
  }
}
