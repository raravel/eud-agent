import { spawn } from "node:child_process";
import {
  copyFile,
  mkdtemp,
  mkdir,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { isDeepStrictEqual } from "node:util";

const PATCH_SCHEMA_NAME = "dat-changes.schema.json";
const PATCH_NAME = "dat-changes.json";
const SQL_PATCH_NAME = "dat-changes.sql";
const BATCH_MCP_SERVER_PATH = resolve("tools/dat-batch-mcp-server.mjs");

function emptyProject() {
  return {
    schemaVersion: 1,
    dat: { units: {}, weapons: {} },
    xdat: { wireframe: {} },
    tbl: {},
    requirements: { units: {} },
    buttons: {},
  };
}

function createScenario(changeCount) {
  const initial = emptyProject();
  const expected = emptyProject();
  const changes = [];
  const taskLines = [];
  const counters = { units: 0, weapons: 0, xdat: 0, tbl: 0, requirements: 0, buttons: 0 };
  const familyCounts = { dat: 0, xdat: 0, tbl: 0, requirements: 0, buttons: 0 };

  const record = (description, change, family) => {
    taskLines.push(`${taskLines.length + 1}. ${description}`);
    changes.push(change);
    familyCounts[family] += 1;
  };

  for (let index = 0; index < changeCount; index += 1) {
    const slot = index % 10;
    if (slot <= 3) {
      const objectId = counters.units++;
      const before = 10240 + objectId * 256;
      const after = before + 2560;
      initial.dat.units[objectId] = { "Hit Points": before };
      expected.dat.units[objectId] = { "Hit Points": after };
      record(
        `units object ${objectId}, Hit Points: ${before} -> ${after}`,
        { kind: "dat", dat: "units", objectId, field: "Hit Points", before, after },
        "dat",
      );
    } else if (slot <= 6) {
      const objectId = counters.weapons++;
      const before = 6 + objectId;
      const after = before + 3;
      initial.dat.weapons[objectId] = { "Damage Amount": before };
      expected.dat.weapons[objectId] = { "Damage Amount": after };
      record(
        `weapons object ${objectId}, Damage Amount: ${before} -> ${after}`,
        { kind: "dat", dat: "weapons", objectId, field: "Damage Amount", before, after },
        "dat",
      );
    } else if (slot === 7) {
      const objectId = counters.xdat++;
      const before = objectId % 10;
      const after = before + 1;
      initial.xdat.wireframe[objectId] = { wire: before };
      expected.xdat.wireframe[objectId] = { wire: after };
      record(
        `wireframe object ${objectId}, wire: ${before} -> ${after}`,
        { kind: "xdat", dat: "wireframe", objectId, field: "wire", before, after },
        "xdat",
      );
    } else if (slot === 8) {
      const indexId = counters.tbl++;
      const before = `Unit ${indexId}`;
      const after = `정예 유닛 ${indexId}`;
      initial.tbl[indexId] = before;
      expected.tbl[indexId] = after;
      record(
        `TBL index ${indexId}: "${before}" -> "${after}"`,
        { kind: "tbl", index: indexId, before, after },
        "tbl",
      );
    } else if ((counters.requirements + counters.buttons) % 2 === 0) {
      const objectId = counters.requirements++;
      initial.requirements.units[objectId] = "0";
      expected.requirements.units[objectId] = "3";
      record(
        `units requirements object ${objectId}: "0" -> "3"`,
        { kind: "requirement", dat: "units", objectId, before: "0", after: "3" },
        "requirements",
      );
    } else {
      const setId = counters.buttons++;
      const before = "1,2,3,4,5,6,7,8";
      const after = `1,2,3,4,5,6,${9 + setId},${10 + setId}`;
      initial.buttons[setId] = before;
      expected.buttons[setId] = after;
      record(
        `button set ${setId} CSV: "${before}" -> "${after}"`,
        { kind: "button", setId, before, after },
        "buttons",
      );
    }
  }

  return {
    changeCount,
    initial,
    expected,
    changes,
    familyCounts,
    task: `Apply exactly these ${changeCount} changes and preserve every other value:\n${taskLines.join("\n")}`,
  };
}

function objectSchema(properties, required) {
  return {
    type: "object",
    properties,
    required,
    additionalProperties: false,
  };
}

function patchSchema(changeCount) {
  const integer = { type: "integer", minimum: 0 };
  const text = { type: "string" };
  const numericChange = (kind) => objectSchema(
    {
      kind: { const: kind },
      dat: { type: "string", minLength: 1 },
      objectId: integer,
      field: { type: "string", minLength: 1 },
      before: { type: "integer" },
      after: { type: "integer" },
    },
    ["kind", "dat", "objectId", "field", "before", "after"],
  );
  return {
    $schema: "https://json-schema.org/draft/2020-12/schema",
    $id: "https://eud-agent.local/schemas/dat-changes-v1.json",
    title: "EUD DAT changes",
    type: "object",
    properties: {
      $schema: { const: `./${PATCH_SCHEMA_NAME}` },
      schemaVersion: { const: 1 },
      changes: {
        type: "array",
        minItems: changeCount,
        maxItems: changeCount,
        items: {
          oneOf: [
            numericChange("dat"),
            numericChange("xdat"),
            objectSchema(
              { kind: { const: "tbl" }, index: integer, before: text, after: text },
              ["kind", "index", "before", "after"],
            ),
            objectSchema(
              {
                kind: { const: "requirement" },
                dat: { type: "string", minLength: 1 },
                objectId: integer,
                before: text,
                after: text,
              },
              ["kind", "dat", "objectId", "before", "after"],
            ),
            objectSchema(
              { kind: { const: "button" }, setId: integer, before: text, after: text },
              ["kind", "setId", "before", "after"],
            ),
          ],
        },
      },
    },
    required: ["$schema", "schemaVersion", "changes"],
    additionalProperties: false,
  };
}

function closedKeys(value, allowed, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  const actual = Object.keys(value).sort();
  const expected = [...allowed].sort();
  if (!isDeepStrictEqual(actual, expected)) {
    throw new Error(`${label} keys ${JSON.stringify(actual)} != ${JSON.stringify(expected)}`);
  }
}

function requireInteger(value, label) {
  if (!Number.isInteger(value)) throw new Error(`${label} must be an integer`);
}

function targetKey(change) {
  switch (change.kind) {
    case "dat":
    case "xdat":
      return `${change.kind}|${change.dat}|${change.objectId}|${change.field}`;
    case "tbl":
      return `tbl|${change.index}`;
    case "requirement":
      return `requirement|${change.dat}|${change.objectId}`;
    case "button":
      return `button|${change.setId}`;
    default:
      throw new Error(`unknown change kind: ${change.kind}`);
  }
}

function applyJsonPatch(initial, patch, expectedCount) {
  closedKeys(patch, ["$schema", "schemaVersion", "changes"], "patch");
  if (patch.$schema !== `./${PATCH_SCHEMA_NAME}`) throw new Error("patch $schema mismatch");
  if (patch.schemaVersion !== 1) throw new Error("patch schemaVersion mismatch");
  if (!Array.isArray(patch.changes) || patch.changes.length !== expectedCount) {
    throw new Error(`patch must contain exactly ${expectedCount} changes`);
  }

  const project = structuredClone(initial);
  const seen = new Set();
  for (const [position, change] of patch.changes.entries()) {
    const label = `changes[${position}]`;
    let current;
    switch (change.kind) {
      case "dat":
      case "xdat":
        closedKeys(change, ["kind", "dat", "objectId", "field", "before", "after"], label);
        requireInteger(change.objectId, `${label}.objectId`);
        requireInteger(change.before, `${label}.before`);
        requireInteger(change.after, `${label}.after`);
        current = project[change.kind]?.[change.dat]?.[String(change.objectId)]?.[change.field];
        if (current !== change.before) throw new Error(`${label} stale before value`);
        project[change.kind][change.dat][String(change.objectId)][change.field] = change.after;
        break;
      case "tbl":
        closedKeys(change, ["kind", "index", "before", "after"], label);
        requireInteger(change.index, `${label}.index`);
        if (typeof change.before !== "string" || typeof change.after !== "string") {
          throw new Error(`${label} TBL values must be strings`);
        }
        current = project.tbl?.[String(change.index)];
        if (current !== change.before) throw new Error(`${label} stale before value`);
        project.tbl[String(change.index)] = change.after;
        break;
      case "requirement":
        closedKeys(change, ["kind", "dat", "objectId", "before", "after"], label);
        requireInteger(change.objectId, `${label}.objectId`);
        if (typeof change.before !== "string" || typeof change.after !== "string") {
          throw new Error(`${label} requirement values must be strings`);
        }
        current = project.requirements?.[change.dat]?.[String(change.objectId)];
        if (current !== change.before) throw new Error(`${label} stale before value`);
        project.requirements[change.dat][String(change.objectId)] = change.after;
        break;
      case "button":
        closedKeys(change, ["kind", "setId", "before", "after"], label);
        requireInteger(change.setId, `${label}.setId`);
        if (typeof change.before !== "string" || typeof change.after !== "string") {
          throw new Error(`${label} button values must be strings`);
        }
        current = project.buttons?.[String(change.setId)];
        if (current !== change.before) throw new Error(`${label} stale before value`);
        project.buttons[String(change.setId)] = change.after;
        break;
      default:
        throw new Error(`${label} unknown kind`);
    }
    if (change.before === change.after) throw new Error(`${label} is a no-op`);
    const key = targetKey(change);
    if (seen.has(key)) throw new Error(`${label} duplicates ${key}`);
    seen.add(key);
  }
  return project;
}

const VALIDATE_PATCH_SCRIPT = `import { readFile } from "node:fs/promises";
+
+const [patchPath, currentPath, countText] = process.argv.slice(2);
+const expectedCount = Number(countText);
+const patch = JSON.parse(await readFile(patchPath, "utf8"));
+const current = JSON.parse(await readFile(currentPath, "utf8"));
+const exactKeys = (value, allowed, label) => {
+  if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error(label + " must be an object");
+  const actual = Object.keys(value).sort().join("|");
+  const expected = [...allowed].sort().join("|");
+  if (actual !== expected) throw new Error(label + " has unexpected keys");
+};
+exactKeys(patch, ["$schema", "schemaVersion", "changes"], "patch");
+if (patch.$schema !== "./dat-changes.schema.json" || patch.schemaVersion !== 1) throw new Error("patch schema header mismatch");
+if (!Array.isArray(patch.changes) || patch.changes.length !== expectedCount) throw new Error("patch change count mismatch");
+const seen = new Set();
+for (const [position, change] of patch.changes.entries()) {
+  const label = "changes[" + position + "]";
+  let key;
+  let value;
+  if (change.kind === "dat" || change.kind === "xdat") {
+    exactKeys(change, ["kind", "dat", "objectId", "field", "before", "after"], label);
+    key = [change.kind, change.dat, change.objectId, change.field].join("|");
+    value = current[change.kind]?.[change.dat]?.[String(change.objectId)]?.[change.field];
+    if (!Number.isInteger(change.before) || !Number.isInteger(change.after)) throw new Error(label + " values must be integers");
+  } else if (change.kind === "tbl") {
+    exactKeys(change, ["kind", "index", "before", "after"], label);
+    key = ["tbl", change.index].join("|");
+    value = current.tbl?.[String(change.index)];
+  } else if (change.kind === "requirement") {
+    exactKeys(change, ["kind", "dat", "objectId", "before", "after"], label);
+    key = ["requirement", change.dat, change.objectId].join("|");
+    value = current.requirements?.[change.dat]?.[String(change.objectId)];
+  } else if (change.kind === "button") {
+    exactKeys(change, ["kind", "setId", "before", "after"], label);
+    key = ["button", change.setId].join("|");
+    value = current.buttons?.[String(change.setId)];
+  } else throw new Error(label + " unknown kind");
+  if (value !== change.before) throw new Error(label + " stale before value");
+  if (change.before === change.after) throw new Error(label + " is a no-op");
+  if (seen.has(key)) throw new Error(label + " duplicate target");
+  seen.add(key);
+}
+console.log(JSON.stringify({ ok: true, changes: patch.changes.length }));
+`;

const SQL_SCHEMA = `PRAGMA foreign_keys = ON;
CREATE TABLE numeric_values (
  kind TEXT NOT NULL CHECK (kind IN ('dat', 'xdat')),
  dat TEXT NOT NULL,
  object_id INTEGER NOT NULL CHECK (object_id >= 0),
  field TEXT NOT NULL,
  value INTEGER NOT NULL,
  PRIMARY KEY (kind, dat, object_id, field)
) STRICT;
CREATE TABLE tbl_strings (
  index_id INTEGER PRIMARY KEY CHECK (index_id >= 0),
  value TEXT NOT NULL
) STRICT;
CREATE TABLE requirements (
  dat TEXT NOT NULL,
  object_id INTEGER NOT NULL CHECK (object_id >= 0),
  payload TEXT NOT NULL,
  PRIMARY KEY (dat, object_id)
) STRICT;
CREATE TABLE buttons (
  set_id INTEGER PRIMARY KEY CHECK (set_id >= 0),
  csv TEXT NOT NULL
) STRICT;
`;

function initializeDatabase(path, scenario) {
  const db = new DatabaseSync(path);
  db.exec(SQL_SCHEMA);
  db.exec("BEGIN IMMEDIATE");
  const numeric = db.prepare(
    "INSERT INTO numeric_values(kind, dat, object_id, field, value) VALUES (?, ?, ?, ?, ?)",
  );
  const tbl = db.prepare("INSERT INTO tbl_strings(index_id, value) VALUES (?, ?)");
  const requirement = db.prepare(
    "INSERT INTO requirements(dat, object_id, payload) VALUES (?, ?, ?)",
  );
  const button = db.prepare("INSERT INTO buttons(set_id, csv) VALUES (?, ?)");
  for (const change of scenario.changes) {
    if (change.kind === "dat" || change.kind === "xdat") {
      numeric.run(change.kind, change.dat, change.objectId, change.field, change.before);
    } else if (change.kind === "tbl") {
      tbl.run(change.index, change.before);
    } else if (change.kind === "requirement") {
      requirement.run(change.dat, change.objectId, change.before);
    } else if (change.kind === "button") {
      button.run(change.setId, change.before);
    }
  }
  db.exec("COMMIT");
  db.close();
}

function splitSql(source) {
  const withoutComments = source
    .split(/\r?\n/)
    .map((line) => line.replace(/--.*$/, ""))
    .join("\n");
  return withoutComments
    .split(";")
    .map((statement) => statement.trim())
    .filter(Boolean);
}

function validateSqlShape(source, expectedCount) {
  const statements = splitSql(source);
  if (statements.length !== expectedCount + 2) {
    throw new Error(`SQL patch needs BEGIN + ${expectedCount} UPDATEs + COMMIT`);
  }
  if (!/^BEGIN\s+IMMEDIATE$/i.test(statements[0])) {
    throw new Error("SQL patch must start with BEGIN IMMEDIATE");
  }
  if (!/^COMMIT$/i.test(statements.at(-1))) {
    throw new Error("SQL patch must end with COMMIT");
  }
  for (const [index, statement] of statements.slice(1, -1).entries()) {
    if (!/^UPDATE\s+/i.test(statement)) {
      throw new Error(`SQL statement ${index + 1} is not UPDATE`);
    }
  }
  return statements;
}

function executeSqlPatch(dbPath, source, expectedCount) {
  const statements = validateSqlShape(source, expectedCount);
  const db = new DatabaseSync(dbPath);
  db.exec("BEGIN IMMEDIATE");
  try {
    for (const [index, statement] of statements.slice(1, -1).entries()) {
      db.exec(statement);
      const row = db.prepare("SELECT changes() AS count").get();
      if (Number(row.count) !== 1) {
        throw new Error(`SQL UPDATE ${index + 1} affected ${row.count} rows`);
      }
    }
    db.exec("COMMIT");
  } catch (error) {
    db.exec("ROLLBACK");
    db.close();
    throw error;
  }
  db.close();
}

function databaseProject(path) {
  const project = emptyProject();
  const db = new DatabaseSync(path, { readOnly: true });
  for (const row of db.prepare(
    "SELECT kind, dat, object_id, field, value FROM numeric_values ORDER BY kind, dat, object_id, field",
  ).all()) {
    project[row.kind][row.dat][String(row.object_id)] = {
      [row.field]: Number(row.value),
    };
  }
  for (const row of db.prepare("SELECT index_id, value FROM tbl_strings ORDER BY index_id").all()) {
    project.tbl[String(row.index_id)] = row.value;
  }
  for (const row of db.prepare(
    "SELECT dat, object_id, payload FROM requirements ORDER BY dat, object_id",
  ).all()) {
    project.requirements[row.dat][String(row.object_id)] = row.payload;
  }
  for (const row of db.prepare("SELECT set_id, csv FROM buttons ORDER BY set_id").all()) {
    project.buttons[String(row.set_id)] = row.csv;
  }
  db.close();
  return project;
}

const DBCTL_SCRIPT = `import { copyFile, readFile, rm } from "node:fs/promises";
+import { DatabaseSync } from "node:sqlite";
+
+const [command, ...args] = process.argv.slice(2);
+const dbPath = "project.sqlite";
+if (command === "query") {
+  const sql = args.join(" ").trim();
+  if (!/^SELECT\\s+/i.test(sql)) throw new Error("query accepts SELECT only");
+  const db = new DatabaseSync(dbPath, { readOnly: true });
+  const rows = db.prepare(sql).all();
+  db.close();
+  console.log(JSON.stringify(rows));
+} else if (command === "validate") {
+  const [patchPath, countText] = args;
+  const expectedCount = Number(countText);
+  const source = await readFile(patchPath, "utf8");
+  const statements = source.split(/;/).map((value) => value.replace(/--.*$/gm, "").trim()).filter(Boolean);
+  if (statements.length !== expectedCount + 2) throw new Error("statement count mismatch");
+  if (!/^BEGIN\\s+IMMEDIATE$/i.test(statements[0])) throw new Error("missing BEGIN IMMEDIATE");
+  if (!/^COMMIT$/i.test(statements.at(-1))) throw new Error("missing COMMIT");
+  if (statements.slice(1, -1).some((statement) => !/^UPDATE\\s+/i.test(statement))) throw new Error("only UPDATE statements are allowed");
+  const validationPath = ".validation.sqlite";
+  await copyFile(dbPath, validationPath);
+  const db = new DatabaseSync(validationPath);
+  db.exec("BEGIN IMMEDIATE");
+  try {
+    for (const [index, statement] of statements.slice(1, -1).entries()) {
+      db.exec(statement);
+      const row = db.prepare("SELECT changes() AS count").get();
+      if (Number(row.count) !== 1) throw new Error("UPDATE " + (index + 1) + " affected " + row.count + " rows");
+    }
+    db.exec("COMMIT");
+  } catch (error) {
+    db.exec("ROLLBACK");
+    throw error;
+  } finally {
+    db.close();
+    await rm(validationPath, { force: true });
+  }
+  console.log(JSON.stringify({ ok: true, updates: expectedCount }));
+} else {
+  throw new Error("usage: node dbctl.mjs query <SELECT> | validate <patch.sql> <count>");
+}
+`;

function sqlString(value) {
  return `'${String(value).replaceAll("'", "''")}'`;
}

function canonicalSqlPatch(scenario) {
  const updates = scenario.changes.map((change) => {
    if (change.kind === "dat" || change.kind === "xdat") {
      return `UPDATE numeric_values SET value = ${change.after} WHERE kind = ${sqlString(change.kind)} AND dat = ${sqlString(change.dat)} AND object_id = ${change.objectId} AND field = ${sqlString(change.field)} AND value = ${change.before}`;
    }
    if (change.kind === "tbl") {
      return `UPDATE tbl_strings SET value = ${sqlString(change.after)} WHERE index_id = ${change.index} AND value = ${sqlString(change.before)}`;
    }
    if (change.kind === "requirement") {
      return `UPDATE requirements SET payload = ${sqlString(change.after)} WHERE dat = ${sqlString(change.dat)} AND object_id = ${change.objectId} AND payload = ${sqlString(change.before)}`;
    }
    return `UPDATE buttons SET csv = ${sqlString(change.after)} WHERE set_id = ${change.setId} AND csv = ${sqlString(change.before)}`;
  });
  return `BEGIN IMMEDIATE;\n${updates.join(";\n")};\nCOMMIT;\n`;
}

function schemaJsonPrompt(scenario) {
  return `This is a controlled schema-backed JSON patch benchmark. ${scenario.task}
+
+Read current-project.json and dat-changes.schema.json. Write only dat-changes.json with exactly
+${scenario.changeCount} closed-union change objects. Every object must contain the exact before
+and after values and no extra properties. Do not modify the current project, schema, or validator.
+After writing the patch, run:
+node validate-patch.mjs dat-changes.json current-project.json ${scenario.changeCount}
+Stop after validation succeeds.`;
}

function sqliteSqlPrompt(scenario) {
  return `This is a controlled SQLite SQL patch benchmark. ${scenario.task}
+
+Read schema.sql. project.sqlite is the current authority; use read-only SELECT queries through
+node dbctl.mjs query "<SELECT>" when needed. Write only dat-changes.sql. It must contain BEGIN
+IMMEDIATE, exactly ${scenario.changeCount} UPDATE statements, and COMMIT. Every UPDATE WHERE
+clause must include the exact before value so stale data affects zero rows. Do not modify the
+database, schema, or helper. After writing the patch, run:
+node dbctl.mjs validate dat-changes.sql ${scenario.changeCount}
+Stop after validation succeeds.`;
}

function batchMcpPrompt(scenario) {
  return `This is a controlled schema-rich batch MCP benchmark. ${scenario.task}

The project state is opaque. Use only the dat_batch MCP tools. Batch all reads into the smallest
possible dat_get, xdat_get, tbl_get, req_get, and btn_get calls. Then call dat_patch exactly once
with all ${scenario.changeCount} closed-union changes. Every target must be read before dat_patch.
Preserve every unmentioned value and stop after dat_patch succeeds.`;
}

function parseOptions(argv) {
  let method;
  let changes = 50;
  let runs = 3;
  let timeoutSeconds = 1200;
  let model;
  let output;
  let allowUnsandboxed = false;
  let selfCheck = false;
  for (const argument of argv) {
    if (argument.startsWith("--method=")) method = argument.slice("--method=".length);
    else if (argument.startsWith("--changes=")) changes = Number(argument.slice("--changes=".length));
    else if (argument.startsWith("--runs=")) runs = Number(argument.slice("--runs=".length));
    else if (argument.startsWith("--timeout-seconds=")) timeoutSeconds = Number(argument.slice("--timeout-seconds=".length));
    else if (argument.startsWith("--model=")) model = argument.slice("--model=".length);
    else if (argument.startsWith("--output=")) output = argument.slice("--output=".length);
    else if (argument === "--allow-unsandboxed") allowUnsandboxed = true;
    else if (argument === "--self-check") selfCheck = true;
    else throw new Error(`unknown argument: ${argument}`);
  }
  if (!method || !["schema-json", "sqlite-sql", "mcp-batch"].includes(method)) {
    throw new Error("--method must be schema-json, sqlite-sql, or mcp-batch");
  }
  if (!Number.isInteger(changes) || changes < 1 || changes > 500) throw new Error("--changes must be in 1..500");
  if (!Number.isInteger(runs) || runs < 1 || runs > 10) throw new Error("--runs must be in 1..10");
  if (!Number.isInteger(timeoutSeconds) || timeoutSeconds < 30 || timeoutSeconds > 1800) throw new Error("--timeout-seconds must be in 30..1800");
  if (!allowUnsandboxed && !selfCheck) {
    throw new Error("benchmark disables the Codex sandbox; pass --allow-unsandboxed only for generated temporary workspaces");
  }
  return { method, changes, runs, timeoutMs: timeoutSeconds * 1000, model, output, allowUnsandboxed, selfCheck };
}

function spawnCodex(cwd, prompt, model, timeoutMs, mcpConfig = null) {
  const windowsCodex = join(dirname(process.execPath), "node_modules", "@openai", "codex", "bin", "codex.js");
  const executable = process.platform === "win32" ? process.execPath : "codex";
  const args = [
    ...(process.platform === "win32" ? [windowsCodex] : []),
    "exec",
    "--ignore-rules",
    "--ignore-user-config",
    "--skip-git-repo-check",
    "--ephemeral",
    "--dangerously-bypass-approvals-and-sandbox",
    "--json",
    "--color",
    "never",
    "-C",
    cwd,
  ];
  if (mcpConfig) {
    args.push(
      "-c",
      `mcp_servers.dat_batch.command=${JSON.stringify(process.execPath)}`,
      "-c",
      `mcp_servers.dat_batch.args=${JSON.stringify([
        mcpConfig.serverPath,
        cwd,
        String(mcpConfig.expectedCount),
      ])}`,
      "-c",
      "mcp_servers.dat_batch.startup_timeout_sec=20",
    );
  }
  if (model) args.push("--model", model);
  args.push(prompt);

  return new Promise((resolvePromise, reject) => {
    const started = performance.now();
    const child = spawn(executable, args, {
      cwd,
      windowsHide: true,
      env: { ...process.env, NO_COLOR: "1" },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      if (process.platform === "win32") {
        spawn("taskkill.exe", ["/PID", String(child.pid), "/T", "/F"], {
          stdio: "ignore",
          windowsHide: true,
        }).unref();
      } else child.kill("SIGTERM");
    }, timeoutMs);
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      resolvePromise({
        code,
        signal,
        timedOut,
        elapsedMs: Math.round(performance.now() - started),
        stdout,
        stderr,
      });
    });
  });
}

function parseMetrics(stdout) {
  let commandExecutions = 0;
  let fileChanges = 0;
  let mcpToolCalls = 0;
  let inputTokens = null;
  let cachedInputTokens = null;
  let outputTokens = null;
  let malformedEventLines = 0;
  for (const line of stdout.split(/\r?\n/)) {
    if (!line.trim()) continue;
    let event;
    try {
      event = JSON.parse(line);
    } catch {
      malformedEventLines += 1;
      continue;
    }
    if (event.type === "item.completed") {
      if (event.item?.type === "command_execution") commandExecutions += 1;
      if (event.item?.type === "file_change") fileChanges += 1;
      if (event.item?.type === "mcp_tool_call") mcpToolCalls += 1;
    }
    if (event.usage) {
      inputTokens = event.usage.input_tokens ?? inputTokens;
      cachedInputTokens = event.usage.cached_input_tokens ?? cachedInputTokens;
      outputTokens = event.usage.output_tokens ?? outputTokens;
    }
  }
  return {
    commandExecutions,
    fileChanges,
    mcpToolCalls,
    inputTokens,
    cachedInputTokens,
    outputTokens,
    malformedEventLines,
  };
}

async function setupSchemaJson(root, scenario) {
  const currentText = JSON.stringify(scenario.initial, null, 2) + "\n";
  const schemaText = JSON.stringify(patchSchema(scenario.changeCount), null, 2) + "\n";
  const validatorText = VALIDATE_PATCH_SCRIPT.replace(/^\+/gm, "");
  await writeFile(join(root, "current-project.json"), currentText, "utf8");
  await writeFile(join(root, PATCH_SCHEMA_NAME), schemaText, "utf8");
  await writeFile(join(root, "validate-patch.mjs"), validatorText, "utf8");
  await writeFile(
    join(root, PATCH_NAME),
    JSON.stringify({ $schema: `./${PATCH_SCHEMA_NAME}`, schemaVersion: 1, changes: [] }, null, 2) + "\n",
    "utf8",
  );
  return {
    prompt: schemaJsonPrompt(scenario),
    fixture: { currentText, schemaText, validatorText },
  };
}

async function evaluateSchemaJson(root, scenario, fixture) {
  const patchResult = await readJsonFile(join(root, PATCH_NAME));
  let finalProject = null;
  let error = patchResult.error;
  if (patchResult.value) {
    try {
      finalProject = applyJsonPatch(scenario.initial, patchResult.value, scenario.changeCount);
    } catch (caught) {
      error = String(caught);
    }
  }
  const fixtureIntact =
    (await readFile(join(root, "current-project.json"), "utf8")) === fixture.currentText &&
    (await readFile(join(root, PATCH_SCHEMA_NAME), "utf8")) === fixture.schemaText &&
    (await readFile(join(root, "validate-patch.mjs"), "utf8")) === fixture.validatorText;
  return {
    artifactValid: !error,
    fixtureIntact,
    stateMatches: isDeepStrictEqual(finalProject, scenario.expected),
    operationCount: patchResult.value?.changes?.length ?? 0,
    validationError: error,
  };
}

async function setupSqliteSql(root, scenario) {
  const dbPath = join(root, "project.sqlite");
  initializeDatabase(dbPath, scenario);
  const dbBytes = await readFile(dbPath);
  const helperText = DBCTL_SCRIPT.replace(/^\+/gm, "");
  await writeFile(join(root, "schema.sql"), SQL_SCHEMA, "utf8");
  await writeFile(join(root, "dbctl.mjs"), helperText, "utf8");
  await writeFile(join(root, SQL_PATCH_NAME), "BEGIN IMMEDIATE;\n\nCOMMIT;\n", "utf8");
  return {
    prompt: sqliteSqlPrompt(scenario),
    fixture: { dbBytes, helperText },
  };
}

async function evaluateSqliteSql(root, scenario, fixture) {
  const patchPath = join(root, SQL_PATCH_NAME);
  const source = await readFile(patchPath, "utf8");
  const evaluationPath = join(root, ".evaluation.sqlite");
  let finalProject = null;
  let error = null;
  let operationCount = 0;
  try {
    operationCount = validateSqlShape(source, scenario.changeCount).length - 2;
    await copyFile(join(root, "project.sqlite"), evaluationPath);
    executeSqlPatch(evaluationPath, source, scenario.changeCount);
    finalProject = databaseProject(evaluationPath);
  } catch (caught) {
    error = String(caught);
  } finally {
    await rm(evaluationPath, { force: true });
  }
  const fixtureIntact =
    (await readFile(join(root, "schema.sql"), "utf8")) === SQL_SCHEMA &&
    (await readFile(join(root, "dbctl.mjs"), "utf8")) === fixture.helperText &&
    (await readFile(join(root, "project.sqlite"))).equals(fixture.dbBytes);
  return {
    artifactValid: !error,
    fixtureIntact,
    stateMatches: isDeepStrictEqual(finalProject, scenario.expected),
    operationCount,
    validationError: error,
  };
}

async function readAudit(path) {
  try {
    const source = await readFile(path, "utf8");
    return source
      .split(/\r?\n/)
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  } catch {
    return [];
  }
}

function validateBatchAudit(audit, scenario) {
  const writes = audit.filter((entry) => entry.op === "write");
  const commits = audit.filter((entry) => entry.op === "commit");
  if (writes.length !== scenario.changeCount) {
    throw new Error(`batch audit has ${writes.length} writes`);
  }
  if (commits.length !== 1 || commits[0].count !== scenario.changeCount) {
    throw new Error("batch audit must contain one complete commit");
  }
  const expectedTargets = scenario.changes.map(targetKey).sort();
  const actualTargets = writes.map((entry) => entry.target).sort();
  if (!isDeepStrictEqual(actualTargets, expectedTargets)) {
    throw new Error("batch audit write targets mismatch");
  }
  for (const target of expectedTargets) {
    const readIndex = audit.findIndex((entry) => entry.op === "read" && entry.target === target);
    const writeIndex = audit.findIndex((entry) => entry.op === "write" && entry.target === target);
    if (readIndex < 0 || writeIndex <= readIndex) {
      throw new Error(`batch audit read-before-write failed for ${target}`);
    }
  }
  return writes.length;
}

async function setupBatchMcp(root, scenario) {
  const stateText = JSON.stringify(scenario.initial, null, 2) + "\n";
  const serverText = await readFile(BATCH_MCP_SERVER_PATH, "utf8");
  await writeFile(join(root, ".tool-state.json"), stateText, "utf8");
  return {
    prompt: batchMcpPrompt(scenario),
    fixture: { serverText },
    mcpConfig: {
      serverPath: BATCH_MCP_SERVER_PATH,
      expectedCount: scenario.changeCount,
    },
  };
}

async function evaluateBatchMcp(root, scenario, fixture) {
  const stateResult = await readJsonFile(join(root, ".tool-state.json"));
  const audit = await readAudit(join(root, ".tool-audit.jsonl"));
  let operationCount = 0;
  let error = stateResult.error;
  try {
    operationCount = validateBatchAudit(audit, scenario);
  } catch (caught) {
    error = String(caught);
  }
  const fixtureIntact =
    (await readFile(BATCH_MCP_SERVER_PATH, "utf8")) === fixture.serverText;
  return {
    artifactValid: !error,
    fixtureIntact,
    stateMatches: isDeepStrictEqual(stateResult.value, scenario.expected),
    operationCount,
    auditOperations: audit.length,
    validationError: error,
  };
}

async function readJsonFile(path) {
  try {
    return { value: JSON.parse(await readFile(path, "utf8")), error: null };
  } catch (error) {
    return { value: null, error: String(error) };
  }
}

async function runTrial(method, run, scenario, options) {
  const root = await mkdtemp(join(tmpdir(), `eud-dat-patch-${method}-${scenario.changeCount}-${run}-`));
  try {
    const setup = method === "schema-json"
      ? await setupSchemaJson(root, scenario)
      : method === "sqlite-sql"
        ? await setupSqliteSql(root, scenario)
        : await setupBatchMcp(root, scenario);
    const processResult = await spawnCodex(
      root,
      setup.prompt,
      options.model,
      options.timeoutMs,
      setup.mcpConfig,
    );
    const evaluation = method === "schema-json"
      ? await evaluateSchemaJson(root, scenario, setup.fixture)
      : method === "sqlite-sql"
        ? await evaluateSqliteSql(root, scenario, setup.fixture)
        : await evaluateBatchMcp(root, scenario, setup.fixture);
    const completed = !processResult.timedOut && processResult.code === 0 && processResult.signal === null;
    return {
      method,
      run,
      changeCount: scenario.changeCount,
      completed,
      success:
        completed &&
        evaluation.artifactValid &&
        evaluation.fixtureIntact &&
        evaluation.stateMatches,
      ...evaluation,
      elapsedMs: processResult.elapsedMs,
      exitCode: processResult.code,
      signal: processResult.signal,
      timedOut: processResult.timedOut,
      ...parseMetrics(processResult.stdout),
      stderr: processResult.stderr.trim() || null,
    };
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

function aggregate(trials) {
  const numeric = (value) => typeof value === "number" && Number.isFinite(value);
  const values = (field) => trials.map((trial) => trial[field]).filter(numeric).sort((a, b) => a - b);
  const average = (field) => {
    const items = values(field);
    return items.length ? Math.round(items.reduce((sum, value) => sum + value, 0) / items.length) : null;
  };
  const median = (field) => {
    const items = values(field);
    if (!items.length) return null;
    const middle = Math.floor(items.length / 2);
    return items.length % 2 ? items[middle] : Math.round((items[middle - 1] + items[middle]) / 2);
  };
  return {
    runs: trials.length,
    successes: trials.filter((trial) => trial.success).length,
    successRate: trials.filter((trial) => trial.success).length / trials.length,
    averageElapsedMs: average("elapsedMs"),
    medianElapsedMs: median("elapsedMs"),
    averageCommandExecutions: average("commandExecutions"),
    averageFileChanges: average("fileChanges"),
    averageMcpToolCalls: average("mcpToolCalls"),
    averageAuditOperations: average("auditOperations"),
    averageInputTokens: average("inputTokens"),
    medianInputTokens: median("inputTokens"),
    averageCachedInputTokens: average("cachedInputTokens"),
    averageOutputTokens: average("outputTokens"),
    medianOutputTokens: median("outputTokens"),
  };
}

async function selfCheck(options, scenario) {
  if (scenario.changes.length !== scenario.changeCount) throw new Error("scenario count mismatch");
  const patch = {
    $schema: `./${PATCH_SCHEMA_NAME}`,
    schemaVersion: 1,
    changes: scenario.changes,
  };
  if (options.method === "schema-json" || options.method === "mcp-batch") {
    const applied = applyJsonPatch(scenario.initial, patch, scenario.changeCount);
    if (!isDeepStrictEqual(applied, scenario.expected)) throw new Error("canonical JSON patch mismatch");
  } else {
    const root = await mkdtemp(join(tmpdir(), "eud-dat-sql-self-check-"));
    try {
      const dbPath = join(root, "project.sqlite");
      initializeDatabase(dbPath, scenario);
      executeSqlPatch(dbPath, canonicalSqlPatch(scenario), scenario.changeCount);
      if (!isDeepStrictEqual(databaseProject(dbPath), scenario.expected)) {
        throw new Error("canonical SQL patch mismatch");
      }
    } finally {
      await rm(root, { recursive: true, force: true });
    }
  }
  return {
    schemaVersion: 1,
    method: options.method,
    changeCount: scenario.changeCount,
    familyCounts: scenario.familyCounts,
    validation: { operations: scenario.changes.length, stateMatches: true },
  };
}

const options = parseOptions(process.argv.slice(2));
const scenario = createScenario(options.changes);
let result;
if (options.selfCheck) {
  result = await selfCheck(options, scenario);
} else {
  const trials = [];
  for (let run = 1; run <= options.runs; run += 1) {
    trials.push(await runTrial(options.method, run, scenario, options));
  }
  result = {
    schemaVersion: 1,
    benchmark: "dat-authoring-patch",
    unsandboxedCodex: options.allowUnsandboxed,
    model: options.model ?? null,
    method: options.method,
    changeCount: scenario.changeCount,
    familyCounts: scenario.familyCounts,
    aggregate: aggregate(trials),
    trials,
  };
}
const output = JSON.stringify(result, null, 2) + "\n";
if (options.output) {
  const outputPath = isAbsolute(options.output) ? options.output : resolve(options.output);
  await mkdir(dirname(outputPath), { recursive: true });
  await writeFile(outputPath, output, "utf8");
}
process.stdout.write(output);
