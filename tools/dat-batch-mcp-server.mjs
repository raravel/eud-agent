import { appendFile, readFile, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";

const [root, countText] = process.argv.slice(2);
const expectedCount = Number(countText);
if (!root || !Number.isInteger(expectedCount) || expectedCount < 1) {
  throw new Error("usage: node dat-batch-mcp-server.mjs <root> <expected-count>");
}
const statePath = join(root, ".tool-state.json");
const auditPath = join(root, ".tool-audit.jsonl");

const integer = { type: "integer", minimum: 0 };
const text = { type: "string" };
const namedText = { type: "string", minLength: 1 };
const object = (properties, required) => ({
  type: "object",
  properties,
  required,
  additionalProperties: false,
});
const batch = (properties, required) => object(
  {
    items: {
      type: "array",
      minItems: 1,
      items: object(properties, required),
    },
  },
  ["items"],
);
const numericChange = (kind) => object(
  {
    kind: { const: kind },
    dat: namedText,
    objectId: integer,
    field: namedText,
    before: { type: "integer" },
    after: { type: "integer" },
  },
  ["kind", "dat", "objectId", "field", "before", "after"],
);

const tools = [
  {
    name: "dat_get",
    description: "Read one or more standard DAT values.",
    inputSchema: batch({ dat: namedText, param: namedText, objId: integer }, ["dat", "param", "objId"]),
  },
  {
    name: "xdat_get",
    description: "Read one or more XDAT values.",
    inputSchema: batch({ dat: namedText, name: namedText, objId: integer }, ["dat", "name", "objId"]),
  },
  {
    name: "tbl_get",
    description: "Read one or more TBL strings.",
    inputSchema: batch({ index: integer }, ["index"]),
  },
  {
    name: "req_get",
    description: "Read one or more requirement payloads.",
    inputSchema: batch({ dat: namedText, objId: integer }, ["dat", "objId"]),
  },
  {
    name: "btn_get",
    description: "Read one or more button CSV payloads.",
    inputSchema: batch({ setId: integer }, ["setId"]),
  },
  {
    name: "dat_patch",
    description: "Atomically validate and stage one complete DAT changeset.",
    inputSchema: object(
      {
        changes: {
          type: "array",
          minItems: expectedCount,
          maxItems: expectedCount,
          items: {
            oneOf: [
              numericChange("dat"),
              numericChange("xdat"),
              object(
                { kind: { const: "tbl" }, index: integer, before: text, after: text },
                ["kind", "index", "before", "after"],
              ),
              object(
                {
                  kind: { const: "requirement" },
                  dat: namedText,
                  objectId: integer,
                  before: text,
                  after: text,
                },
                ["kind", "dat", "objectId", "before", "after"],
              ),
              object(
                { kind: { const: "button" }, setId: integer, before: text, after: text },
                ["kind", "setId", "before", "after"],
              ),
            ],
          },
        },
      },
      ["changes"],
    ),
  },
];

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

function exactKeys(value, allowed, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  const actual = Object.keys(value).sort().join("|");
  const expected = [...allowed].sort().join("|");
  if (actual !== expected) throw new Error(`${label} has unexpected keys`);
}

function requireValue(value, label) {
  if (value === undefined) throw new Error(`${label} not found`);
  return value;
}

async function appendAudit(entries) {
  await appendFile(
    auditPath,
    entries.map((entry) => JSON.stringify(entry)).join("\n") + "\n",
    "utf8",
  );
}

async function readState() {
  return JSON.parse(await readFile(statePath, "utf8"));
}

async function writeState(state) {
  const temporary = `${statePath}.tmp`;
  await writeFile(temporary, JSON.stringify(state, null, 2) + "\n", "utf8");
  await rename(temporary, statePath);
}

async function batchRead(name, items) {
  const state = await readState();
  const entries = [];
  const results = items.map((item) => {
    let value;
    let target;
    if (name === "dat_get") {
      target = `dat|${item.dat}|${item.objId}|${item.param}`;
      value = state.dat?.[item.dat]?.[String(item.objId)]?.[item.param];
    } else if (name === "xdat_get") {
      target = `xdat|${item.dat}|${item.objId}|${item.name}`;
      value = state.xdat?.[item.dat]?.[String(item.objId)]?.[item.name];
    } else if (name === "tbl_get") {
      target = `tbl|${item.index}`;
      value = state.tbl?.[String(item.index)];
    } else if (name === "req_get") {
      target = `requirement|${item.dat}|${item.objId}`;
      value = state.requirements?.[item.dat]?.[String(item.objId)];
    } else {
      target = `button|${item.setId}`;
      value = state.buttons?.[String(item.setId)];
    }
    entries.push({ op: "read", target });
    return { ...item, value: requireValue(value, target) };
  });
  await appendAudit(entries);
  return { count: results.length, results };
}

async function applyPatch(args) {
  exactKeys(args, ["changes"], "dat_patch arguments");
  if (!Array.isArray(args.changes) || args.changes.length !== expectedCount) {
    throw new Error(`dat_patch requires exactly ${expectedCount} changes`);
  }
  const state = await readState();
  const draft = structuredClone(state);
  const seen = new Set();
  const writes = [];

  for (const [position, change] of args.changes.entries()) {
    const label = `changes[${position}]`;
    const target = targetKey(change);
    if (seen.has(target)) throw new Error(`${label} duplicates ${target}`);
    seen.add(target);
    if (change.before === change.after) throw new Error(`${label} is a no-op`);

    let current;
    if (change.kind === "dat" || change.kind === "xdat") {
      exactKeys(change, ["kind", "dat", "objectId", "field", "before", "after"], label);
      current = draft[change.kind]?.[change.dat]?.[String(change.objectId)]?.[change.field];
      if (current !== change.before) throw new Error(`${label} stale before value`);
      draft[change.kind][change.dat][String(change.objectId)][change.field] = change.after;
    } else if (change.kind === "tbl") {
      exactKeys(change, ["kind", "index", "before", "after"], label);
      current = draft.tbl?.[String(change.index)];
      if (current !== change.before) throw new Error(`${label} stale before value`);
      draft.tbl[String(change.index)] = change.after;
    } else if (change.kind === "requirement") {
      exactKeys(change, ["kind", "dat", "objectId", "before", "after"], label);
      current = draft.requirements?.[change.dat]?.[String(change.objectId)];
      if (current !== change.before) throw new Error(`${label} stale before value`);
      draft.requirements[change.dat][String(change.objectId)] = change.after;
    } else if (change.kind === "button") {
      exactKeys(change, ["kind", "setId", "before", "after"], label);
      current = draft.buttons?.[String(change.setId)];
      if (current !== change.before) throw new Error(`${label} stale before value`);
      draft.buttons[String(change.setId)] = change.after;
    }
    writes.push({ op: "write", target });
  }

  await writeState(draft);
  await appendAudit([...writes, { op: "commit", count: writes.length }]);
  return { ok: true, changes: writes.length };
}

function textResult(value, isError = false) {
  return {
    content: [{ type: "text", text: JSON.stringify(value) }],
    isError,
  };
}

async function callTool(name, args) {
  if (["dat_get", "xdat_get", "tbl_get", "req_get", "btn_get"].includes(name)) {
    return batchRead(name, args.items);
  }
  if (name === "dat_patch") return applyPatch(args);
  throw new Error(`unknown tool: ${name}`);
}

function send(message) {
  process.stdout.write(JSON.stringify(message) + "\n");
}

async function handle(message) {
  if (!Object.prototype.hasOwnProperty.call(message, "id")) return;
  try {
    let result;
    if (message.method === "initialize") {
      result = {
        protocolVersion: message.params?.protocolVersion ?? "2025-06-18",
        capabilities: { tools: { listChanged: false } },
        serverInfo: { name: "eud-dat-batch-benchmark", version: "1.0.0" },
      };
    } else if (message.method === "tools/list") {
      result = { tools };
    } else if (message.method === "tools/call") {
      try {
        result = textResult(await callTool(message.params?.name, message.params?.arguments ?? {}));
      } catch (error) {
        result = textResult({ error: String(error) }, true);
      }
    } else if (message.method === "ping") {
      result = {};
    } else {
      throw new Error(`unsupported method: ${message.method}`);
    }
    send({ jsonrpc: "2.0", id: message.id, result });
  } catch (error) {
    send({
      jsonrpc: "2.0",
      id: message.id,
      error: { code: -32601, message: String(error) },
    });
  }
}

let buffer = "";
let queue = Promise.resolve();
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  for (;;) {
    const newline = buffer.indexOf("\n");
    if (newline < 0) break;
    const line = buffer.slice(0, newline).trim();
    buffer = buffer.slice(newline + 1);
    if (!line) continue;
    queue = queue.then(() => handle(JSON.parse(line)));
  }
});
