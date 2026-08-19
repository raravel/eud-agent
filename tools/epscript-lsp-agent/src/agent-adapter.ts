import {
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { TextDocument } from "vscode-languageserver-textdocument";
import { URI } from "vscode-uri";
import { CharStreams, CommonTokenStream } from "antlr4ts";
import { ParseTreeListener } from "antlr4ts/tree/ParseTreeListener";
import { ParseTreeWalker } from "antlr4ts/tree/ParseTreeWalker";
import { Analyzer } from "#upstream/analyzer";
import { epScriptLexer } from "#upstream/grammar/lib/epScriptLexer";
import {
  epScriptParser,
  ImportStatementContext,
} from "#upstream/grammar/lib/epScriptParser";
import { epScriptParserListener } from "#upstream/grammar/lib/epScriptParserListener";
import { LanguageManager } from "#upstream/i18n/LanguageManager";
import { Parser } from "#upstream/parser";

export const MAX_DIAGNOSTICS = 200;
export const MAX_MESSAGE_BYTES = 32 * 1024;
export const MAX_FRAME_BYTES = 8 * 1024 * 1024;

interface Candidate {
  path: string;
  code: string;
}

interface UnreadableFile {
  path: string;
  ftype?: string;
}

interface AnalyzeParams {
  root: string;
  candidates: Candidate[];
  unreadable?: UnreadableFile[];
}

interface ImportLocation {
  line: number;
  character: number;
  endLine: number;
  endCharacter: number;
}

interface ImportEdge extends ImportLocation {
  from: string;
  module: string;
  to: string;
  status: "resolved" | "missing" | "unreadable";
}

interface NormalizedDiagnostic {
  path: string;
  line: number;
  character: number;
  endLine: number;
  endCharacter: number;
  severity: "error" | "warning" | "information" | "hint";
  source: string;
  code: string | number | null;
  message: string;
}

interface ProjectFile {
  path: string;
  absolute: string;
  code: string;
}

function normalizedKey(value: string): string {
  return value.normalize("NFC").toLowerCase();
}

function comparePath(left: string, right: string): number {
  return normalizedKey(left).localeCompare(normalizedKey(right), "en") ||
    left.localeCompare(right, "en");
}

function normalizeProjectPath(value: string): string {
  if (typeof value !== "string" || !value || value.includes("\0")) {
    throw new Error("candidate path must be a non-empty UTF-8 string");
  }
  if (value.includes("\\") || value.startsWith("/") || /^[A-Za-z]:/.test(value)) {
    throw new Error(`candidate path is not normalized: ${value}`);
  }
  const segments = value.split("/");
  if (segments.some((segment) => !segment || segment === "." || segment === "..")) {
    throw new Error(`candidate path contains an invalid segment: ${value}`);
  }
  if (!value.toLowerCase().endsWith(".eps")) {
    throw new Error(`candidate path must end in .eps: ${value}`);
  }
  return segments.join("/");
}

function containedPath(root: string, relative: string): string {
  const absoluteRoot = path.resolve(root);
  const target = path.resolve(absoluteRoot, ...relative.split("/"));
  const relation = path.relative(absoluteRoot, target);
  if (!relation || relation.startsWith("..") || path.isAbsolute(relation)) {
    if (relation) {
      throw new Error(`path escapes analysis root: ${relative}`);
    }
  }
  return target;
}

function collisionDiagnostic(pathValue: string, other: string): NormalizedDiagnostic {
  return {
    path: pathValue,
    line: 1,
    character: 1,
    endLine: 1,
    endCharacter: 1,
    severity: "error",
    source: "eud-agent",
    code: "EUDLSP003",
    message: `project paths collide case-insensitively: ${other} and ${pathValue}`,
  };
}

function overlayCandidates(
  root: string,
  candidates: Candidate[],
): { paths: string[]; diagnostics: NormalizedDiagnostic[] } {
  const seen = new Map<string, string>();
  const accepted: Candidate[] = [];
  const diagnostics: NormalizedDiagnostic[] = [];

  for (const candidate of candidates) {
    const projectPath = normalizeProjectPath(candidate.path);
    if (typeof candidate.code !== "string") {
      throw new Error(`candidate code must be a string: ${projectPath}`);
    }
    const key = normalizedKey(projectPath);
    const previous = seen.get(key);
    if (previous) {
      diagnostics.push(collisionDiagnostic(projectPath, previous));
      continue;
    }
    seen.set(key, projectPath);
    accepted.push({ path: projectPath, code: candidate.code });
  }

  const staged: Array<{ temporary: string; target: string }> = [];
  try {
    accepted.forEach((candidate, index) => {
      const target = containedPath(root, candidate.path);
      mkdirSync(path.dirname(target), { recursive: true });
      const temporary = `${target}.eud-agent-${process.pid}-${index}.tmp`;
      writeFileSync(temporary, candidate.code, { encoding: "utf8", flag: "wx" });
      staged.push({ temporary, target });
    });
    for (const item of staged) {
      rmSync(item.target, { force: true });
      renameSync(item.temporary, item.target);
    }
  } catch (error) {
    for (const item of staged) {
      rmSync(item.temporary, { force: true });
    }
    throw error;
  }

  return { paths: accepted.map((candidate) => candidate.path), diagnostics };
}

function walkProject(root: string): ProjectFile[] {
  const files: ProjectFile[] = [];
  const stack = [path.resolve(root)];
  while (stack.length > 0) {
    const directory = stack.pop()!;
    const entries = readdirSync(directory, { withFileTypes: true }).sort((left, right) =>
      left.name.localeCompare(right.name, "en"),
    );
    for (let index = entries.length - 1; index >= 0; index -= 1) {
      const entry = entries[index];
      const absolute = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        stack.push(absolute);
      } else if (entry.isFile() && entry.name.toLowerCase().endsWith(".eps")) {
        const projectPath = path.relative(root, absolute).split(path.sep).join("/");
        files.push({ path: projectPath, absolute, code: readFileSync(absolute, "utf8") });
      }
    }
  }
  return files.sort((left, right) => comparePath(left.path, right.path));
}

function importLocations(code: string): Array<{ module: string } & ImportLocation> {
  const lexer = new epScriptLexer(CharStreams.fromString(code));
  lexer.removeErrorListeners();
  const tokens = new CommonTokenStream(lexer);
  const parser = new epScriptParser(tokens);
  parser.removeErrorListeners();
  const tree = parser.program();
  const imports: Array<{ module: string } & ImportLocation> = [];
  const listener: epScriptParserListener = {
    enterImportStatement(ctx: ImportStatementContext) {
      const stop = ctx.stop ?? ctx.start;
      const stopText = stop.text ?? "";
      imports.push({
        module: ctx.dottedName().text,
        line: ctx.start.line,
        character: ctx.start.charPositionInLine + 1,
        endLine: stop.line,
        endCharacter: stop.charPositionInLine + Math.max(1, stopText.length) + 1,
      });
    },
  };
  ParseTreeWalker.DEFAULT.walk(listener as ParseTreeListener, tree);
  return imports;
}

function syntheticImportDiagnostic(
  edge: ImportEdge,
  code: "EUDLSP001" | "EUDLSP002" | "EUDLSP004",
  severity: "error" | "warning",
  message: string,
): NormalizedDiagnostic {
  return {
    path: edge.from,
    line: edge.line,
    character: edge.character,
    endLine: edge.endLine,
    endCharacter: edge.endCharacter,
    severity,
    source: "eud-agent",
    code,
    message,
  };
}

function buildGraph(
  files: ProjectFile[],
  unreadable: UnreadableFile[],
): {
  edges: ImportEdge[];
  diagnostics: NormalizedDiagnostic[];
  paths: Map<string, string>;
} {
  const diagnostics: NormalizedDiagnostic[] = [];
  const paths = new Map<string, string>();
  for (const file of files) {
    const key = normalizedKey(file.path);
    const previous = paths.get(key);
    if (previous && previous !== file.path) {
      diagnostics.push(collisionDiagnostic(file.path, previous));
    } else {
      paths.set(key, file.path);
    }
  }

  const unreadablePaths = new Map<string, string>();
  for (const item of unreadable) {
    const projectPath = normalizeProjectPath(item.path);
    const key = normalizedKey(projectPath);
    const previous = paths.get(key) ?? unreadablePaths.get(key);
    if (previous && previous !== projectPath) {
      diagnostics.push(collisionDiagnostic(projectPath, previous));
    } else if (!paths.has(key)) {
      unreadablePaths.set(key, projectPath);
      paths.set(key, projectPath);
    }
  }

  const edges: ImportEdge[] = [];
  for (const file of files) {
    for (const imported of importLocations(file.code)) {
      const requested = `${imported.module.split(".").join("/")}.eps`;
      const target = paths.get(normalizedKey(requested));
      const status = target
        ? unreadablePaths.has(normalizedKey(target))
          ? "unreadable"
          : "resolved"
        : "missing";
      const edge: ImportEdge = {
        from: file.path,
        module: imported.module,
        to: target ?? requested,
        status,
        line: imported.line,
        character: imported.character,
        endLine: imported.endLine,
        endCharacter: imported.endCharacter,
      };
      edges.push(edge);
      if (status === "missing") {
        diagnostics.push(
          syntheticImportDiagnostic(
            edge,
            "EUDLSP001",
            "error",
            `imported module does not exist: ${imported.module} (${requested})`,
          ),
        );
      } else if (status === "unreadable") {
        diagnostics.push(
          syntheticImportDiagnostic(
            edge,
            "EUDLSP004",
            "error",
            `imported project file could not be snapshotted: ${edge.to}`,
          ),
        );
      }
    }
  }

  edges.sort(
    (left, right) =>
      comparePath(left.from, right.from) ||
      left.line - right.line ||
      left.character - right.character ||
      left.module.localeCompare(right.module, "en"),
  );
  return { edges, diagnostics, paths };
}

function adjacencyFor(
  nodes: string[],
  edges: ImportEdge[],
): { forward: Map<string, string[]>; reverse: Map<string, string[]> } {
  const forward = new Map(nodes.map((node) => [node, [] as string[]]));
  const reverse = new Map(nodes.map((node) => [node, [] as string[]]));
  for (const edge of edges) {
    if (edge.status !== "resolved" || !forward.has(edge.from) || !forward.has(edge.to)) {
      continue;
    }
    forward.get(edge.from)!.push(edge.to);
    reverse.get(edge.to)!.push(edge.from);
  }
  for (const values of [...forward.values(), ...reverse.values()]) {
    values.sort(comparePath);
  }
  return { forward, reverse };
}

function affectedClosure(
  candidatePaths: string[],
  forward: Map<string, string[]>,
  reverse: Map<string, string[]>,
): Set<string> {
  const affected = new Set<string>();
  const queue = [...candidatePaths].sort(comparePath);
  while (queue.length > 0) {
    const item = queue.shift()!;
    if (affected.has(item)) {
      continue;
    }
    affected.add(item);
    for (const neighbor of [...(forward.get(item) ?? []), ...(reverse.get(item) ?? [])]) {
      if (!affected.has(neighbor)) {
        queue.push(neighbor);
      }
    }
    queue.sort(comparePath);
  }
  return affected;
}

function stronglyConnected(
  nodes: string[],
  forward: Map<string, string[]>,
  reverse: Map<string, string[]>,
): string[][] {
  const visited = new Set<string>();
  const finished: string[] = [];
  for (const start of [...nodes].sort(comparePath)) {
    if (visited.has(start)) continue;
    const stack: Array<{ node: string; next: number }> = [{ node: start, next: 0 }];
    visited.add(start);
    while (stack.length > 0) {
      const frame = stack[stack.length - 1];
      const neighbors = forward.get(frame.node) ?? [];
      if (frame.next < neighbors.length) {
        const next = neighbors[frame.next++];
        if (!visited.has(next)) {
          visited.add(next);
          stack.push({ node: next, next: 0 });
        }
      } else {
        finished.push(frame.node);
        stack.pop();
      }
    }
  }

  const assigned = new Set<string>();
  const components: string[][] = [];
  for (let index = finished.length - 1; index >= 0; index -= 1) {
    const start = finished[index];
    if (assigned.has(start)) continue;
    const component: string[] = [];
    const stack = [start];
    assigned.add(start);
    while (stack.length > 0) {
      const node = stack.pop()!;
      component.push(node);
      for (const next of reverse.get(node) ?? []) {
        if (!assigned.has(next)) {
          assigned.add(next);
          stack.push(next);
        }
      }
    }
    component.sort(comparePath);
    components.push(component);
  }
  return components;
}

function stableAnalysisOrder(
  components: string[][],
  forward: Map<string, string[]>,
): string[] {
  const componentOf = new Map<string, number>();
  components.forEach((component, index) =>
    component.forEach((node) => componentOf.set(node, index)),
  );
  const dependencies = components.map(() => new Set<number>());
  const importers = components.map(() => new Set<number>());
  for (const [from, targets] of forward) {
    const fromComponent = componentOf.get(from)!;
    for (const target of targets) {
      const targetComponent = componentOf.get(target)!;
      if (fromComponent !== targetComponent) {
        dependencies[fromComponent].add(targetComponent);
        importers[targetComponent].add(fromComponent);
      }
    }
  }

  const ready = components
    .map((_, index) => index)
    .filter((index) => dependencies[index].size === 0)
    .sort((left, right) => comparePath(components[left][0], components[right][0]));
  const ordered: number[] = [];
  while (ready.length > 0) {
    const component = ready.shift()!;
    ordered.push(component);
    for (const importer of importers[component]) {
      dependencies[importer].delete(component);
      if (dependencies[importer].size === 0) {
        ready.push(importer);
        ready.sort((left, right) => comparePath(components[left][0], components[right][0]));
      }
    }
  }
  return ordered.flatMap((index) => components[index]);
}

function severityName(value: unknown): NormalizedDiagnostic["severity"] {
  switch (value) {
    case 2:
      return "warning";
    case 3:
      return "information";
    case 4:
      return "hint";
    default:
      return "error";
  }
}

function normalizeUpstreamDiagnostic(
  projectPath: string,
  diagnostic: unknown,
): NormalizedDiagnostic {
  const object =
    diagnostic !== null && typeof diagnostic === "object"
      ? (diagnostic as Record<string, unknown>)
      : {};
  const range =
    object.range !== null && typeof object.range === "object"
      ? (object.range as Record<string, unknown>)
      : {};
  const start =
    range.start !== null && typeof range.start === "object"
      ? (range.start as Record<string, unknown>)
      : {};
  const end =
    range.end !== null && typeof range.end === "object"
      ? (range.end as Record<string, unknown>)
      : start;
  const line = typeof start.line === "number" && Number.isInteger(start.line) ? start.line : 0;
  const character =
    typeof start.character === "number" && Number.isInteger(start.character)
      ? start.character
      : 0;
  const endLine = typeof end.line === "number" && Number.isInteger(end.line) ? end.line : 0;
  const endCharacter =
    typeof end.character === "number" && Number.isInteger(end.character) ? end.character : 0;
  const code =
    typeof object.code === "string" || typeof object.code === "number" ? object.code : null;
  return {
    path: projectPath,
    line: line + 1,
    character: character + 1,
    endLine: endLine + 1,
    endCharacter: endCharacter + 1,
    severity: severityName(object.severity),
    source: typeof object.source === "string" && object.source ? object.source : "epscript-lsp",
    code,
    message: String(object.message ?? "unknown analyzer diagnostic"),
  };
}

function analyzerFailureDiagnostic(projectPath: string, error: unknown): NormalizedDiagnostic {
  const message = error instanceof Error ? error.message : String(error);
  return {
    path: projectPath,
    line: 1,
    character: 1,
    endLine: 1,
    endCharacter: 1,
    severity: "error",
    source: "epscript-lsp",
    code: null,
    message: `analyzer failed for this file: ${message}`,
  };
}

function capDiagnostics(diagnostics: NormalizedDiagnostic[]): {
  diagnostics: NormalizedDiagnostic[];
  truncated: boolean;
  omittedDiagnostics: number;
  omittedMessageBytes: number;
} {
  const output: NormalizedDiagnostic[] = [];
  let messageBytes = 0;
  let omittedDiagnostics = 0;
  let omittedMessageBytes = 0;

  for (const diagnostic of diagnostics) {
    if (output.length >= MAX_DIAGNOSTICS) {
      omittedDiagnostics += 1;
      omittedMessageBytes += Buffer.byteLength(diagnostic.message, "utf8");
      continue;
    }
    const bytes = Buffer.from(diagnostic.message, "utf8");
    const remaining = MAX_MESSAGE_BYTES - messageBytes;
    if (remaining <= 0) {
      omittedDiagnostics += 1;
      omittedMessageBytes += bytes.length;
      continue;
    }
    let message = diagnostic.message;
    if (bytes.length > remaining) {
      let clipped = bytes.subarray(0, remaining);
      while (clipped.length > 0 && (clipped[clipped.length - 1] & 0xc0) === 0x80) {
        clipped = clipped.subarray(0, clipped.length - 1);
      }
      message = clipped.toString("utf8");
      omittedMessageBytes += bytes.length - Buffer.byteLength(message, "utf8");
    }
    messageBytes += Buffer.byteLength(message, "utf8");
    output.push({ ...diagnostic, message });
  }

  return {
    diagnostics: output,
    truncated: omittedDiagnostics > 0 || omittedMessageBytes > 0,
    omittedDiagnostics,
    omittedMessageBytes,
  };
}

export async function analyzeProject(params: AnalyzeParams) {
  if (!params || typeof params.root !== "string" || !path.isAbsolute(params.root)) {
    throw new Error("analysis root must be an absolute path");
  }
  if (!Array.isArray(params.candidates) || params.candidates.length === 0) {
    throw new Error("at least one complete candidate is required");
  }
  const root = path.resolve(params.root);
  if (!existsSync(root) || !statSync(root).isDirectory()) {
    throw new Error("analysis root does not exist");
  }

  const overlay = overlayCandidates(root, params.candidates);
  const files = walkProject(root);
  const unreadable = Array.isArray(params.unreadable) ? params.unreadable : [];
  const graph = buildGraph(files, unreadable);
  const readablePaths = files.map((file) => file.path);
  const { forward, reverse } = adjacencyFor(readablePaths, graph.edges);
  const actualCandidates = overlay.paths
    .map((candidate) => graph.paths.get(normalizedKey(candidate)) ?? candidate)
    .filter((candidate) => forward.has(candidate));
  const affected = affectedClosure(actualCandidates, forward, reverse);
  const components = stronglyConnected(readablePaths, forward, reverse);

  const diagnostics = [
    ...overlay.diagnostics,
    ...graph.diagnostics.filter(
      (diagnostic) => diagnostic.code === "EUDLSP003" || affected.has(diagnostic.path),
    ),
  ];
  const componentByPath = new Map<string, string[]>();
  for (const component of components) {
    component.forEach((item) => componentByPath.set(item, component));
  }
  for (const edge of graph.edges) {
    if (edge.status !== "resolved" || !affected.has(edge.from)) continue;
    const component = componentByPath.get(edge.from) ?? [];
    if (component.length > 1 && component.includes(edge.to) || edge.from === edge.to) {
      diagnostics.push(
        syntheticImportDiagnostic(
          edge,
          "EUDLSP002",
          "warning",
          `import cycle detected: ${component.length > 1 ? component.join(" -> ") : edge.from}`,
        ),
      );
    }
  }

  const affectedComponents = components
    .map((component) => component.filter((item) => affected.has(item)))
    .filter((component) => component.length > 0);
  const affectedForward = new Map<string, string[]>();
  for (const item of affected) {
    affectedForward.set(
      item,
      (forward.get(item) ?? []).filter((target) => affected.has(target)),
    );
  }
  const order = stableAnalysisOrder(affectedComponents, affectedForward);
  const fileByPath = new Map(files.map((file) => [file.path, file]));
  const analyzer = new Analyzer(Parser.initialize());
  const language = new LanguageManager();

  for (const projectPath of order) {
    const file = fileByPath.get(projectPath);
    if (!file) continue;
    const uri = URI.file(file.absolute).toString();
    try {
      analyzer.analyze(
        uri,
        TextDocument.create(uri, "eps", 0, file.code),
        language,
        root,
        true,
      );
    } catch (error) {
      diagnostics.push(analyzerFailureDiagnostic(projectPath, error));
    }
  }
  for (const projectPath of order) {
    const file = fileByPath.get(projectPath);
    if (!file) continue;
    const uri = URI.file(file.absolute).toString();
    try {
      const analyzed = analyzer.analyze(
        uri,
        TextDocument.create(uri, "eps", 0, file.code),
        language,
        root,
        false,
      );
      diagnostics.push(
        ...analyzed.parsePackage.diagnostic.map((diagnostic) =>
          normalizeUpstreamDiagnostic(projectPath, diagnostic),
        ),
      );
    } catch (error) {
      diagnostics.push(analyzerFailureDiagnostic(projectPath, error));
    }
  }

  diagnostics.sort(
    (left, right) =>
      comparePath(left.path, right.path) ||
      left.line - right.line ||
      left.character - right.character ||
      String(left.code ?? "").localeCompare(String(right.code ?? ""), "en") ||
      left.message.localeCompare(right.message, "en"),
  );
  const capped = capDiagnostics(diagnostics);
  const imports = graph.edges
    .filter((edge) => affected.has(edge.from))
    .map(({ line: _line, character: _character, endLine: _endLine, endCharacter: _endCharacter, ...edge }) => edge);

  return {
    checkedFiles: [...affected].sort(comparePath),
    diagnostics: capped.diagnostics,
    imports,
    truncated: capped.truncated,
    omittedDiagnostics: capped.omittedDiagnostics,
    omittedMessageBytes: capped.omittedMessageBytes,
  };
}

function protocolLog(...values: unknown[]): void {
  process.stderr.write(`${values.map((value) => String(value)).join(" ")}\n`);
}

function writeFrame(value: unknown): void {
  const body = Buffer.from(JSON.stringify(value), "utf8");
  process.stdout.write(`Content-Length: ${body.length}\r\n\r\n`);
  process.stdout.write(body);
}

function startProtocol(): void {
  console.log = protocolLog;
  console.info = protocolLog;
  console.debug = protocolLog;
  let buffer = Buffer.alloc(0);
  let queue = Promise.resolve();

  process.stdin.on("data", (chunk: Buffer) => {
    buffer = Buffer.concat([buffer, chunk]);
    try {
      while (true) {
        const separator = buffer.indexOf("\r\n\r\n");
        if (separator < 0) {
          if (buffer.length > 8192) throw new Error("protocol header is too large");
          break;
        }
        const header = buffer.subarray(0, separator).toString("ascii");
        const match = /^Content-Length: ([0-9]+)$/im.exec(header);
        if (!match) throw new Error("invalid Content-Length header");
        const length = Number(match[1]);
        if (!Number.isSafeInteger(length) || length <= 0 || length > MAX_FRAME_BYTES) {
          throw new Error("invalid Content-Length value");
        }
        const frameEnd = separator + 4 + length;
        if (buffer.length < frameEnd) break;
        const payload = buffer.subarray(separator + 4, frameEnd);
        buffer = buffer.subarray(frameEnd);
        const request = JSON.parse(payload.toString("utf8"));
        queue = queue.then(async () => {
          if (!Number.isSafeInteger(request.id) || request.method !== "analyze") {
            throw new Error("invalid adapter request");
          }
          try {
            const result = await analyzeProject(request.params);
            writeFrame({ id: request.id, result });
          } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            writeFrame({ id: request.id, error: { message } });
          }
        });
      }
    } catch (error) {
      protocolLog(error);
      process.exit(2);
    }
  });
}

if (require.main === module) {
  startProtocol();
}
