import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { isDeepStrictEqual } from "node:util";

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
  const taskLines = [];
  const toolPairs = [];
  const counters = { units: 0, weapons: 0, xdat: 0, tbl: 0, requirements: 0, buttons: 0 };
  const familyCounts = { dat: 0, xdat: 0, tbl: 0, requirements: 0, buttons: 0 };

  const record = (description, pair, family) => {
    taskLines.push(`${taskLines.length + 1}. ${description}`);
    toolPairs.push(pair);
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
        [
          "dat_get",
          ["units", String(objectId), "Hit Points"],
          "dat_set",
          ["units", String(objectId), "Hit Points", String(after)],
        ],
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
        [
          "dat_get",
          ["weapons", String(objectId), "Damage Amount"],
          "dat_set",
          ["weapons", String(objectId), "Damage Amount", String(after)],
        ],
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
        [
          "xdat_get",
          ["wireframe", String(objectId), "wire"],
          "xdat_set",
          ["wireframe", String(objectId), "wire", String(after)],
        ],
        "xdat",
      );
    } else if (slot === 8) {
      const objectId = counters.tbl++;
      const before = `Unit ${objectId}`;
      const after = `정예 유닛 ${objectId}`;
      initial.tbl[objectId] = before;
      expected.tbl[objectId] = after;
      record(
        `TBL index ${objectId}: "${before}" -> "${after}"`,
        [
          "tbl_get",
          [String(objectId)],
          "tbl_set",
          [String(objectId), after],
        ],
        "tbl",
      );
    } else if ((counters.requirements + counters.buttons) % 2 === 0) {
      const objectId = counters.requirements++;
      initial.requirements.units[objectId] = "0";
      expected.requirements.units[objectId] = "3";
      record(
        `units requirements object ${objectId}: "0" -> "3"`,
        [
          "req_get",
          ["units", String(objectId)],
          "req_set",
          ["units", String(objectId), "3"],
        ],
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
        [
          "btn_get",
          [String(setId)],
          "btn_set",
          [String(setId), after],
        ],
        "buttons",
      );
    }
  }

  return {
    changeCount,
    initial,
    expected,
    task: `Apply exactly these ${changeCount} changes and preserve every other value:\n${taskLines.join("\n")}`,
    toolPairs,
    familyCounts,
  };
}

const MCP_SCRIPT = `import { appendFile, readFile, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";

const root = process.argv[2];
const statePath = join(root, ".tool-state.json");
const logPath = join(root, ".tool-audit.jsonl");
const integer = { type: "integer", minimum: 0 };
const string = { type: "string", minLength: 1 };
const object = (properties, required) => ({
  type: "object",
  properties,
  required,
  additionalProperties: false,
});
const batch = (properties, required) =>
  object(
    {
      items: {
        type: "array",
        minItems: 1,
        items: object(properties, required),
      },
    },
    ["items"],
  );
const tools = [
  {
    name: "dat_get",
    description: "Read one or more standard DAT values.",
    inputSchema: batch({ dat: string, param: string, objId: integer }, ["dat", "param", "objId"]),
  },
  {
    name: "xdat_get",
    description: "Read one or more XDAT values.",
    inputSchema: batch({ dat: string, name: string, objId: integer }, ["dat", "name", "objId"]),
  },
  {
    name: "tbl_get",
    description: "Read one or more TBL strings.",
    inputSchema: batch({ index: integer }, ["index"]),
  },
  {
    name: "req_get",
    description: "Read one or more requirement payloads.",
    inputSchema: batch({ dat: string, objId: integer }, ["dat", "objId"]),
  },
  {
    name: "btn_get",
    description: "Read one or more button CSV payloads.",
    inputSchema: batch({ setId: integer }, ["setId"]),
  },
  {
    name: "dat_set",
    description: "Write one standard DAT value.",
    inputSchema: object(
      { dat: string, param: string, objId: integer, value: { type: "integer" } },
      ["dat", "param", "objId", "value"],
    ),
  },
  {
    name: "xdat_set",
    description: "Write one XDAT value.",
    inputSchema: object(
      { dat: string, name: string, objId: integer, value: { type: "integer" } },
      ["dat", "name", "objId", "value"],
    ),
  },
  {
    name: "tbl_set",
    description: "Write one TBL string.",
    inputSchema: object({ index: integer, value: string }, ["index", "value"]),
  },
  {
    name: "req_set",
    description: "Write one requirement payload.",
    inputSchema: object({ dat: string, objId: integer, payload: string }, ["dat", "objId", "payload"]),
  },
  {
    name: "btn_set",
    description: "Write one button CSV payload.",
    inputSchema: object({ setId: integer, csv: string }, ["setId", "csv"]),
  },
];

const requireValue = (value, label) => {
  if (value === undefined) throw new Error(label + " not found");
  return value;
};
const readState = async () => JSON.parse(await readFile(statePath, "utf8"));
const writeState = async (state) => {
  const temporary = statePath + ".tmp";
  await writeFile(temporary, JSON.stringify(state, null, 2) + "\\n", "utf8");
  await rename(temporary, statePath);
};
const appendAudit = async (entries) => {
  await appendFile(
    logPath,
    entries.map((entry) => JSON.stringify(entry)).join("\\n") + "\\n",
    "utf8",
  );
};
const textResult = (value, isError = false) => ({
  content: [{ type: "text", text: JSON.stringify(value) }],
  isError,
});

async function callTool(name, args) {
  const state = await readState();
  let value;
  let audit;
  switch (name) {
    case "dat_get": {
      const results = args.items.map((item) => ({
        ...item,
        value: requireValue(
          state.dat?.[item.dat]?.[String(item.objId)]?.[item.param],
          "DAT target",
        ),
      }));
      audit = args.items.map((item) => ({
        op: "dat_get",
        args: [item.dat, String(item.objId), item.param],
      }));
      value = { count: results.length, results };
      break;
    }
    case "xdat_get": {
      const results = args.items.map((item) => ({
        ...item,
        value: requireValue(
          state.xdat?.[item.dat]?.[String(item.objId)]?.[item.name],
          "XDAT target",
        ),
      }));
      audit = args.items.map((item) => ({
        op: "xdat_get",
        args: [item.dat, String(item.objId), item.name],
      }));
      value = { count: results.length, results };
      break;
    }
    case "tbl_get": {
      const results = args.items.map((item) => ({
        ...item,
        value: requireValue(state.tbl?.[String(item.index)], "TBL target"),
      }));
      audit = args.items.map((item) => ({ op: "tbl_get", args: [String(item.index)] }));
      value = { count: results.length, results };
      break;
    }
    case "req_get": {
      const results = args.items.map((item) => ({
        ...item,
        value: requireValue(
          state.requirements?.[item.dat]?.[String(item.objId)],
          "requirement target",
        ),
      }));
      audit = args.items.map((item) => ({
        op: "req_get",
        args: [item.dat, String(item.objId)],
      }));
      value = { count: results.length, results };
      break;
    }
    case "btn_get": {
      const results = args.items.map((item) => ({
        ...item,
        csv: requireValue(state.buttons?.[String(item.setId)], "button target"),
      }));
      audit = args.items.map((item) => ({ op: "btn_get", args: [String(item.setId)] }));
      value = { count: results.length, results };
      break;
    }
    case "dat_set":
      requireValue(state.dat?.[args.dat]?.[String(args.objId)]?.[args.param], "DAT target");
      state.dat[args.dat][String(args.objId)][args.param] = args.value;
      audit = [{ op: "dat_set", args: [args.dat, String(args.objId), args.param, String(args.value)] }];
      value = { ok: true, value: args.value };
      await writeState(state);
      break;
    case "xdat_set":
      requireValue(state.xdat?.[args.dat]?.[String(args.objId)]?.[args.name], "XDAT target");
      state.xdat[args.dat][String(args.objId)][args.name] = args.value;
      audit = [{ op: "xdat_set", args: [args.dat, String(args.objId), args.name, String(args.value)] }];
      value = { ok: true, value: args.value };
      await writeState(state);
      break;
    case "tbl_set":
      requireValue(state.tbl?.[String(args.index)], "TBL target");
      state.tbl[String(args.index)] = args.value;
      audit = [{ op: "tbl_set", args: [String(args.index), args.value] }];
      value = { ok: true, value: args.value };
      await writeState(state);
      break;
    case "req_set":
      requireValue(state.requirements?.[args.dat]?.[String(args.objId)], "requirement target");
      state.requirements[args.dat][String(args.objId)] = args.payload;
      audit = [{ op: "req_set", args: [args.dat, String(args.objId), args.payload] }];
      value = { ok: true, value: args.payload };
      await writeState(state);
      break;
    case "btn_set":
      requireValue(state.buttons?.[String(args.setId)], "button target");
      state.buttons[String(args.setId)] = args.csv;
      audit = [{ op: "btn_set", args: [String(args.setId), args.csv] }];
      value = { ok: true, value: args.csv };
      await writeState(state);
      break;
    default:
      throw new Error("unknown tool: " + name);
  }
  await appendAudit(audit);
  return value;
}

const send = (message) => process.stdout.write(JSON.stringify(message) + "\\n");
async function handle(message) {
  if (!Object.prototype.hasOwnProperty.call(message, "id")) return;
  try {
    let result;
    if (message.method === "initialize") {
      result = {
        protocolVersion: message.params?.protocolVersion ?? "2025-06-18",
        capabilities: { tools: { listChanged: false } },
        serverInfo: { name: "eud-dat-benchmark", version: "1.0.0" },
      };
    } else if (message.method === "tools/list") {
      result = { tools };
    } else if (message.method === "tools/call") {
      try {
        result = textResult(
          await callTool(message.params?.name, message.params?.arguments ?? {}),
        );
      } catch (error) {
        result = textResult({ error: String(error) }, true);
      }
    } else if (message.method === "ping") {
      result = {};
    } else {
      throw new Error("unsupported method: " + message.method);
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
    const newline = buffer.indexOf("\\n");
    if (newline < 0) break;
    const line = buffer.slice(0, newline).trim();
    buffer = buffer.slice(newline + 1);
    if (!line) continue;
    queue = queue.then(() => handle(JSON.parse(line)));
  }
});
`;

function filePrompt(scenario) {
  return `This is a controlled DAT-authoring benchmark. ${scenario.task}

Use ordinary filesystem read/edit/write capability to inspect and edit project.json.
The JSON document is the only authority. You may use a one-shot shell command to perform the
repetitive edits, but project.json is the only persistent file you may change. Keep it valid,
preserve its structure and every unmentioned value, then stop after verifying project.json.`;
}

function toolPrompt(scenario) {
  return `This is a controlled DAT-authoring benchmark. ${scenario.task}

The project state is opaque. Do not inspect or edit .tool-state.json or .tool-audit.jsonl.
Use only the dat_bench MCP tools. Batch all reads into the smallest possible dat_get, xdat_get,
tbl_get, req_get, and btn_get calls, then use the corresponding individual *_set tools. Every
target must be read before it is written. Preserve every unmentioned value and stop after all
writes succeed.`;
}

function parseOptions(argv) {
  let runs = 3;
  let changes = 50;
  let timeoutSeconds = 600;
  let model;
  let allowUnsandboxed = false;
  let selfCheck = false;
  for (const argument of argv) {
    if (argument.startsWith("--runs=")) runs = Number(argument.slice("--runs=".length));
    else if (argument.startsWith("--changes=")) {
      changes = Number(argument.slice("--changes=".length));
    } else if (argument.startsWith("--timeout-seconds=")) {
      timeoutSeconds = Number(argument.slice("--timeout-seconds=".length));
    } else if (argument.startsWith("--model=")) model = argument.slice("--model=".length);
    else if (argument === "--allow-unsandboxed") allowUnsandboxed = true;
    else if (argument === "--self-check") selfCheck = true;
    else throw new Error(`unknown argument: ${argument}`);
  }
  if (!Number.isInteger(runs) || runs < 1 || runs > 10) {
    throw new Error("--runs must be an integer in 1..10");
  }
  if (!Number.isInteger(changes) || changes < 1 || changes > 500) {
    throw new Error("--changes must be an integer in 1..500");
  }
  if (!Number.isInteger(timeoutSeconds) || timeoutSeconds < 30 || timeoutSeconds > 1800) {
    throw new Error("--timeout-seconds must be an integer in 30..1800");
  }
  if (!allowUnsandboxed && !selfCheck) {
    throw new Error(
      "this controlled benchmark disables the Codex sandbox; rerun with --allow-unsandboxed only in its generated temporary workspaces",
    );
  }
  return {
    runs,
    changes,
    timeoutMs: timeoutSeconds * 1000,
    model,
    allowUnsandboxed,
    selfCheck,
  };
}

function spawnCodex(cwd, prompt, model, timeoutMs, useMcp) {
  const windowsCodex = join(
    dirname(process.execPath),
    "node_modules",
    "@openai",
    "codex",
    "bin",
    "codex.js",
  );
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
  if (useMcp) {
    const serverPath = join(cwd, "dat-mcp.mjs");
    args.push(
      "-c",
      `mcp_servers.dat_bench.command=${JSON.stringify(process.execPath)}`,
      "-c",
      `mcp_servers.dat_bench.args=${JSON.stringify([serverPath, cwd])}`,
      "-c",
      "mcp_servers.dat_bench.startup_timeout_sec=20",
    );
  }
  if (model) args.push("--model", model);
  args.push(prompt);

  return new Promise((resolve, reject) => {
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
        spawn(
          "taskkill.exe",
          ["/PID", String(child.pid), "/T", "/F"],
          { stdio: "ignore", windowsHide: true },
        ).unref();
      } else {
        child.kill("SIGTERM");
      }
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
      resolve({
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

function parseEvents(stdout) {
  const events = [];
  const malformed = [];
  for (const line of stdout.split(/\r?\n/)) {
    if (!line.trim()) continue;
    try {
      events.push(JSON.parse(line));
    } catch {
      malformed.push(line);
    }
  }
  return { events, malformed };
}

function eventMetrics(events) {
  const eventTypes = {};
  let commandExecutions = 0;
  let fileChanges = 0;
  let mcpToolCalls = 0;
  let inputTokens = null;
  let cachedInputTokens = null;
  let outputTokens = null;

  for (const event of events) {
    eventTypes[event.type] = (eventTypes[event.type] ?? 0) + 1;
    const item = event.item ?? event.params?.item;
    const itemType = item?.type;
    if (event.type === "item.completed") {
      if (itemType === "command_execution") commandExecutions += 1;
      if (itemType === "file_change") fileChanges += 1;
      if (itemType === "mcp_tool_call") mcpToolCalls += 1;
    }
    const usage = event.usage ?? event.params?.usage;
    if (usage) {
      inputTokens = usage.input_tokens ?? usage.inputTokens ?? inputTokens;
      cachedInputTokens = usage.cached_input_tokens ?? usage.cachedInputTokens ?? cachedInputTokens;
      outputTokens = usage.output_tokens ?? usage.outputTokens ?? outputTokens;
    }
  }

  return {
    eventTypes,
    commandExecutions,
    fileChanges,
    mcpToolCalls,
    inputTokens,
    cachedInputTokens,
    outputTokens,
  };
}

async function readJson(path) {
  try {
    return { value: JSON.parse(await readFile(path, "utf8")), error: null };
  } catch (error) {
    return { value: null, error: String(error) };
  }
}

async function readAudit(path) {
  try {
    const text = await readFile(path, "utf8");
    return text
      .split(/\r?\n/)
      .filter(Boolean)
      .map((line) => JSON.parse(line));
  } catch {
    return [];
  }
}

function sameArgs(actual, expected) {
  return isDeepStrictEqual(actual.map(String), expected.map(String));
}

function validToolAudit(audit, toolPairs) {
  const writes = audit.filter((entry) => entry.op.endsWith("_set"));
  if (writes.length !== toolPairs.length) return false;
  return toolPairs.every(([getOp, getArgs, setOp, setArgs]) => {
    const getIndex = audit.findIndex(
      (entry) => entry.op === getOp && sameArgs(entry.args, getArgs),
    );
    const setIndex = audit.findIndex(
      (entry) => entry.op === setOp && sameArgs(entry.args, setArgs),
    );
    return getIndex >= 0 && setIndex > getIndex;
  });
}

function countLeafDifferences(left, right) {
  if (
    left !== null &&
    right !== null &&
    typeof left === "object" &&
    typeof right === "object" &&
    !Array.isArray(left) &&
    !Array.isArray(right)
  ) {
    const keys = new Set([...Object.keys(left), ...Object.keys(right)]);
    let count = 0;
    for (const key of keys) count += countLeafDifferences(left[key], right[key]);
    return count;
  }
  return isDeepStrictEqual(left, right) ? 0 : 1;
}

function validateScenario(scenario) {
  const familyTotal = Object.values(scenario.familyCounts).reduce(
    (sum, count) => sum + count,
    0,
  );
  const leafDifferences = countLeafDifferences(scenario.initial, scenario.expected);
  if (familyTotal !== scenario.changeCount) {
    throw new Error(`family count ${familyTotal} != requested ${scenario.changeCount}`);
  }
  if (scenario.toolPairs.length !== scenario.changeCount) {
    throw new Error(
      `tool pair count ${scenario.toolPairs.length} != requested ${scenario.changeCount}`,
    );
  }
  if (leafDifferences !== scenario.changeCount) {
    throw new Error(
      `leaf difference count ${leafDifferences} != requested ${scenario.changeCount}`,
    );
  }

  const audit = scenario.toolPairs.flatMap(([getOp, getArgs, setOp, setArgs]) => [
    { op: getOp, args: getArgs },
    { op: setOp, args: setArgs },
  ]);
  if (!validToolAudit(audit, scenario.toolPairs)) {
    throw new Error("synthetic valid tool audit was rejected");
  }
  const extraWrite = [...audit, audit.find((entry) => entry.op.endsWith("_set"))];
  if (validToolAudit(extraWrite, scenario.toolPairs)) {
    throw new Error("synthetic extra tool write was accepted");
  }
  return { familyTotal, leafDifferences, toolPairs: scenario.toolPairs.length };
}

async function runTrial(mode, run, scenario, options) {
  const root = await mkdtemp(
    join(tmpdir(), `eud-dat-ab-${scenario.changeCount}-${mode}-${run}-`),
  );
  const stateName = mode === "file" ? "project.json" : ".tool-state.json";
  await writeFile(
    join(root, stateName),
    JSON.stringify(scenario.initial, null, 2) + "\n",
    "utf8",
  );
  if (mode === "tool") await writeFile(join(root, "dat-mcp.mjs"), MCP_SCRIPT, "utf8");

  try {
    const processResult = await spawnCodex(
      root,
      mode === "file" ? filePrompt(scenario) : toolPrompt(scenario),
      options.model,
      options.timeoutMs,
      mode === "tool",
    );
    const parsed = parseEvents(processResult.stdout);
    const finalState = await readJson(join(root, stateName));
    const audit = mode === "tool" ? await readAudit(join(root, ".tool-audit.jsonl")) : [];
    const stateMatches = isDeepStrictEqual(finalState.value, scenario.expected);
    const auditValid =
      mode === "file" || validToolAudit(audit, scenario.toolPairs);
    return {
      mode,
      run,
      changeCount: scenario.changeCount,
      completed:
        !processResult.timedOut &&
        processResult.code === 0 &&
        processResult.signal === null,
      success:
        !processResult.timedOut &&
        processResult.code === 0 &&
        stateMatches &&
        auditValid,
      stateMatches,
      auditValid,
      auditOperations: audit.length,
      elapsedMs: processResult.elapsedMs,
      exitCode: processResult.code,
      signal: processResult.signal,
      timedOut: processResult.timedOut,
      finalStateError: finalState.error,
      malformedEventLines: parsed.malformed.length,
      ...eventMetrics(parsed.events),
      stderr: processResult.stderr.trim() || null,
    };
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}

function aggregate(trials) {
  const present = (value) => typeof value === "number" && Number.isFinite(value);
  const average = (field) => {
    const values = trials.map((trial) => trial[field]).filter(present);
    return values.length
      ? Math.round(values.reduce((sum, value) => sum + value, 0) / values.length)
      : null;
  };
  const median = (field) => {
    const values = trials
      .map((trial) => trial[field])
      .filter(present)
      .sort((left, right) => left - right);
    if (!values.length) return null;
    const middle = Math.floor(values.length / 2);
    return values.length % 2 === 1
      ? values[middle]
      : Math.round((values[middle - 1] + values[middle]) / 2);
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

const options = parseOptions(process.argv.slice(2));
const scenario = createScenario(options.changes);
const scenarioValidation = validateScenario(scenario);
if (options.selfCheck) {
  console.log(
    JSON.stringify(
      {
        schemaVersion: 1,
        changeCount: scenario.changeCount,
        familyCounts: scenario.familyCounts,
        validation: scenarioValidation,
      },
      null,
      2,
    ),
  );
} else {
  const trials = [];
  for (let run = 1; run <= options.runs; run += 1) {
    const order = run % 2 === 1 ? ["file", "tool"] : ["tool", "file"];
    for (const mode of order) {
      trials.push(await runTrial(mode, run, scenario, options));
    }
  }
  const fileTrials = trials.filter((trial) => trial.mode === "file");
  const toolTrials = trials.filter((trial) => trial.mode === "tool");
  console.log(
    JSON.stringify(
      {
        schemaVersion: 1,
        unsandboxedCodex: options.allowUnsandboxed,
        changeCount: scenario.changeCount,
        familyCounts: scenario.familyCounts,
        file: aggregate(fileTrials),
        tool: aggregate(toolTrials),
        trials,
      },
      null,
      2,
    ),
  );
}
