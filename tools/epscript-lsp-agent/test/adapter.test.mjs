import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const bundlePath = path.resolve(
  path.dirname(new URL(import.meta.url).pathname.slice(1)),
  "../../../vendor/epscript-lsp-agent/adapter.cjs",
);
const { analyzeProject, MAX_DIAGNOSTICS, MAX_MESSAGE_BYTES } = require(bundlePath);

async function fixture(files = {}) {
  const root = await mkdtemp(path.join(os.tmpdir(), "eud-eps-agent-"));
  for (const [projectPath, code] of Object.entries(files)) {
    const target = path.join(root, ...projectPath.split("/"));
    await mkdir(path.dirname(target), { recursive: true });
    await writeFile(target, code, "utf8");
  }
  return root;
}

async function analyze(root, candidates, unreadable = []) {
  return analyzeProject({ root, candidates, unreadable });
}

function diagnosticCodes(result) {
  return result.diagnostics.map((diagnostic) => diagnostic.code).filter(Boolean);
}

async function removeFixture(root) {
  await rm(root, { recursive: true, force: true });
}

function frame(value) {
  const body = Buffer.from(JSON.stringify(value), "utf8");
  return Buffer.concat([
    Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, "ascii"),
    body,
  ]);
}

function runAdapterProcess(params, timeoutMs = 5000) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [bundlePath], {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
    });
    const stdout = [];
    const stderr = [];
    let settled = false;
    const finish = (value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.kill();
      resolve(value);
    };
    child.stdout.on("data", (chunk) => {
      stdout.push(chunk);
      const bytes = Buffer.concat(stdout);
      const separator = bytes.indexOf("\r\n\r\n");
      if (separator < 0) return;
      const match = /^Content-Length: ([0-9]+)$/im.exec(
        bytes.subarray(0, separator).toString("ascii"),
      );
      if (!match) return;
      const length = Number(match[1]);
      if (bytes.length < separator + 4 + length) return;
      const payload = bytes.subarray(separator + 4, separator + 4 + length);
      finish({ kind: "response", value: JSON.parse(payload.toString("utf8")) });
    });
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    child.on("exit", (code) =>
      finish({ kind: "exit", code, stderr: Buffer.concat(stderr).toString("utf8") }),
    );
    const timer = setTimeout(() => finish({ kind: "timeout" }), timeoutMs);
    child.stdin.end(frame({ id: 1, method: "analyze", params }));
  });
}

test("valid and malformed single-file sources return normalized diagnostics", async () => {
  const root = await fixture();
  try {
    const valid = await analyze(root, [
      { path: "main.eps", code: "function onPluginStart() { var value = 1; }" },
    ]);
    assert.equal(valid.diagnostics.length, 0);

    const malformed = await analyze(root, [
      { path: "main.eps", code: "function broken( {" },
    ]);
    assert.ok(malformed.diagnostics.length > 0);
    assert.equal(malformed.diagnostics[0].path, "main.eps");
    assert.ok(malformed.diagnostics[0].line >= 1);
    assert.ok(malformed.diagnostics[0].character >= 1);
    assert.equal(malformed.diagnostics[0].severity, "error");
  } finally {
    await removeFixture(root);
  }
});

test("direct imports and aliases resolve with cross-file symbols", async () => {
  const root = await fixture({
    "lib/units.eps": "object UnitState { var hp; };",
  });
  try {
    const result = await analyze(root, [
      {
        path: "main.eps",
        code: "import lib.units as units; const state = units.UnitState();",
      },
    ]);
    assert.deepEqual(result.imports, [
      { from: "main.eps", module: "lib.units", to: "lib/units.eps", status: "resolved" },
    ]);
    assert.ok(!result.diagnostics.some((item) => item.message.includes("does not exist")));
  } finally {
    await removeFixture(root);
  }
});

test("nested imports include transitive files and attribute malformed imported code", async () => {
  const root = await fixture({
    "lib/units.eps": "import lib.common; object UnitState { var hp; };",
    "lib/common.eps": "function broken( {",
  });
  try {
    const result = await analyze(root, [
      { path: "main.eps", code: "import lib.units; const state = units.UnitState();" },
    ]);
    assert.deepEqual(result.checkedFiles, ["lib/common.eps", "lib/units.eps", "main.eps"]);
    assert.ok(result.imports.some((item) => item.module === "lib.common"));
    assert.ok(result.diagnostics.some((item) => item.path === "lib/common.eps"));
  } finally {
    await removeFixture(root);
  }
});

test("real framed adapter process covers project chain, repair, nested missing import, and mutual batch", async () => {
  const root = await fixture({
    "lib/units.eps": "import lib.common; object UnitState { var hp; };",
    "lib/common.eps": "object CommonState { var value; };",
  });
  try {
    const candidate = [{ path: "main.eps", code: "import lib.units;" }];
    const valid = await runAdapterProcess({ root, candidates: candidate, unreadable: [] });
    assert.equal(valid.kind, "response");
    assert.deepEqual(valid.value.result.checkedFiles, [
      "lib/common.eps",
      "lib/units.eps",
      "main.eps",
    ]);

    await writeFile(path.join(root, "lib/common.eps"), "function broken( {", "utf8");
    const malformed = await runAdapterProcess({ root, candidates: candidate, unreadable: [] });
    assert.equal(malformed.kind, "response");
    assert.ok(
      malformed.value.result.diagnostics.some(
        (diagnostic) => diagnostic.path === "lib/common.eps",
      ),
    );

    await writeFile(path.join(root, "lib/common.eps"), "object CommonState {};", "utf8");
    await writeFile(path.join(root, "lib/units.eps"), "import lib.missing;", "utf8");
    const missing = await runAdapterProcess({ root, candidates: candidate, unreadable: [] });
    assert.equal(missing.kind, "response");
    assert.ok(
      missing.value.result.diagnostics.some(
        (diagnostic) =>
          diagnostic.path === "lib/units.eps" && diagnostic.code === "EUDLSP001",
      ),
    );

    const mutualRoot = await fixture();
    try {
      const mutual = await runAdapterProcess({
        root: mutualRoot,
        candidates: [
          { path: "a.eps", code: "import b; object A {};" },
          { path: "b.eps", code: "import a; object B {};" },
        ],
        unreadable: [],
      });
      assert.equal(mutual.kind, "response");
      assert.equal(
        mutual.value.result.imports.filter((edge) => edge.status === "resolved").length,
        2,
      );
    } finally {
      await removeFixture(mutualRoot);
    }

    const badCandidate = await runAdapterProcess({
      root,
      candidates: [{ path: "main.eps", code: "function broken( {" }],
      unreadable: [],
    });
    assert.equal(badCandidate.kind, "response");
    assert.ok(badCandidate.value.result.diagnostics.some((item) => item.path === "main.eps"));
    const corrected = await runAdapterProcess({
      root,
      candidates: [{ path: "main.eps", code: "function onPluginStart() {}" }],
      unreadable: [],
    });
    assert.equal(corrected.kind, "response");
    assert.ok(!corrected.value.result.diagnostics.some((item) => item.path === "main.eps"));
  } finally {
    await removeFixture(root);
  }
});

test("mutually dependent candidate batch is overlaid before graph construction", async () => {
  const root = await fixture();
  try {
    const result = await analyze(root, [
      { path: "a.eps", code: "import b; object A { var value; };" },
      { path: "b.eps", code: "import a; object B { var value; };" },
    ]);
    assert.equal(result.imports.filter((item) => item.status === "resolved").length, 2);
    assert.ok(diagnosticCodes(result).includes("EUDLSP002"));
    assert.equal(await readFile(path.join(root, "a.eps"), "utf8"), "import b; object A { var value; };");
    assert.equal(await readFile(path.join(root, "b.eps"), "utf8"), "import a; object B { var value; };");
  } finally {
    await removeFixture(root);
  }
});

test("missing direct and nested imports produce EUDLSP001 at the importer", async () => {
  const root = await fixture({
    "lib/units.eps": "import lib.missing; object UnitState { var hp; };",
    "unrelated.eps": "import unrelated.missing;",
  });
  try {
    const result = await analyze(root, [
      { path: "main.eps", code: "import lib.units;" },
    ]);
    const missing = result.diagnostics.find((item) => item.code === "EUDLSP001");
    assert.equal(missing.path, "lib/units.eps");
    assert.equal(missing.line, 1);
    assert.match(missing.message, /lib\/missing\.eps/);
    assert.equal(
      result.diagnostics.filter((item) => item.code === "EUDLSP001").length,
      1,
      "unrelated project diagnostics must not leak outside the affected closure",
    );
  } finally {
    await removeFixture(root);
  }
});

test("cycles and self-imports produce deterministic EUDLSP002 warnings", async () => {
  const root = await fixture({
    "a.eps": "import b;",
    "b.eps": "import a;",
  });
  try {
    const cycle = await analyze(root, [{ path: "a.eps", code: "import b;" }]);
    const warnings = cycle.diagnostics.filter((item) => item.code === "EUDLSP002");
    assert.equal(warnings.length, 2);
    assert.ok(warnings.every((item) => item.severity === "warning"));

    const self = await analyze(root, [{ path: "self.eps", code: "import self;" }]);
    assert.equal(self.diagnostics.filter((item) => item.code === "EUDLSP002").length, 1);
  } finally {
    await removeFixture(root);
  }
});

test("changed dependencies include transitive reverse dependents", async () => {
  const root = await fixture({
    "main.eps": "import lib.units;",
    "feature.eps": "import main;",
    "lib/units.eps": "object UnitState { var hp; };",
  });
  try {
    const result = await analyze(root, [
      { path: "lib/units.eps", code: "object UnitState { var hp; var armor; };" },
    ]);
    assert.deepEqual(result.checkedFiles, ["feature.eps", "lib/units.eps", "main.eps"]);
  } finally {
    await removeFixture(root);
  }
});

test("strings and comments resembling imports do not create graph edges", async () => {
  const root = await fixture();
  try {
    const result = await analyze(root, [
      {
        path: "main.eps",
        code: 'const text = "import fake.module;"; // import fake.comment;\n/* import fake.block; */',
      },
    ]);
    assert.deepEqual(result.imports, []);
    assert.ok(!diagnosticCodes(result).includes("EUDLSP001"));
  } finally {
    await removeFixture(root);
  }
});

test("Korean paths and Unicode identifiers are preserved and resolved", async () => {
  const root = await fixture({
    "라이브러리/공통.eps": "object 상태 { var 체력; };",
  });
  try {
    const result = await analyze(root, [
      {
        path: "메인.eps",
        code: "import 라이브러리.공통 as 공통별칭; const 현재 = 공통별칭.상태();",
      },
    ]);
    assert.deepEqual(result.imports, [
      {
        from: "메인.eps",
        module: "라이브러리.공통",
        to: "라이브러리/공통.eps",
        status: "resolved",
      },
    ]);
    assert.ok(result.checkedFiles.includes("라이브러리/공통.eps"));
  } finally {
    await removeFixture(root);
  }
});

test("diagnostic and message caps report deterministic omitted counts", async () => {
  const root = await fixture();
  try {
    const repeated = Array.from({ length: MAX_DIAGNOSTICS + 30 }, () => "const duplicate = 1;").join("\n");
    const result = await analyze(root, [{ path: "many.eps", code: repeated }]);
    assert.equal(result.truncated, true);
    assert.ok(result.diagnostics.length <= MAX_DIAGNOSTICS);
    assert.ok(result.omittedDiagnostics > 0 || result.omittedMessageBytes > 0);
    assert.ok(
      result.diagnostics.reduce((sum, item) => sum + Buffer.byteLength(item.message), 0) <=
        MAX_MESSAGE_BYTES,
    );
  } finally {
    await removeFixture(root);
  }
});

test("unreadable imported files produce EUDLSP004 without aborting the project", async () => {
  const root = await fixture();
  try {
    const result = await analyze(
      root,
      [{ path: "main.eps", code: "import lib.closed;" }],
      [{ path: "lib/closed.eps", ftype: "CUIEps" }],
    );
    assert.ok(diagnosticCodes(result).includes("EUDLSP004"));
    assert.equal(result.imports[0].status, "unreadable");
  } finally {
    await removeFixture(root);
  }
});

test("known recursive-symbol input is contained by the adapter process", async () => {
  const root = await fixture();
  try {
    const recursive = `${"(".repeat(6000)}1${")".repeat(6000)};`;
    const outcome = await runAdapterProcess({
      root,
      candidates: [{ path: "recursive.eps", code: recursive }],
      unreadable: [],
    });
    assert.notEqual(outcome.kind, "timeout");
    assert.ok(
      outcome.kind === "exit" ||
        (outcome.kind === "response" && (outcome.value.result || outcome.value.error)),
    );
  } finally {
    await removeFixture(root);
  }
});

test("bundle is self-contained and has no epscript package runtime lookup", async () => {
  const bundle = await readFile(bundlePath, "utf8");
  assert.doesNotMatch(bundle, /require\(["']@epscript-lsp\//);
});
