"use strict";

// EUD Agent panel — WS client, state machine, renderers.
// Vanilla ES2020+ (WebView2 = Chromium). No framework, no CDN, no build step.

(function () {
  // ---- constants -------------------------------------------------------
  var RECONNECT_BACKOFF_MS = 2000; // 2s reconnect backoff
  var PREVIEW_MAX_BYTES = 1024 * 1024; // 1MB preview truncation threshold
  var READONLY_TITLE = "읽기 전용 파일 형식";

  // Progress stages the server may report.
  var PROGRESS_LABELS = {
    rag: "컨텍스트 검색 중",
    rag_warmup: "RAG 모델 준비 중",
    codex: "코드 생성 중",
    lsp: "진단 분석 중",
    waiting_build: "빌드 완료 대기 중",
  };

  // States: connecting | retry | ready | working | reviewing | applying | waiting
  var state = "connecting";

  // ---- DOM lookups -----------------------------------------------------
  var els = {};
  function $(id) {
    return document.getElementById(id);
  }
  function cacheEls() {
    var ids = [
      "conn-state", "project-name", "event-log",
      "target-picker", "refresh-list", "new-file-toggle",
      "neweps-row", "neweps-name", "neweps-error",
      "tab-preview", "tab-diff", "tab-edit", "code-lang",
      "panel-preview", "preview-notice", "preview-code",
      "panel-diff", "diff-view", "panel-edit", "edit-code",
      "diagnostics", "diagnostics-dismiss", "diagnostics-list",
      "apply-set", "apply-neweps", "cancel",
      "instruction-input", "use-context", "send",
    ];
    ids.forEach(function (id) {
      els[id] = $(id);
    });
  }

  // ---- runtime data ----------------------------------------------------
  var ws = null;
  var reconnectTimer = null;
  var hasProject = false; // project open (target list populated)
  var currentCode = ""; // full code from the latest `code` event
  var spinnerLogs = {}; // stage -> log line element (for spinner clearing)

  // ---- token / WS URL --------------------------------------------------
  function getToken() {
    var params = new URLSearchParams(location.search);
    return params.get("token") || "";
  }
  function wsUrl() {
    var token = encodeURIComponent(getToken());
    return "ws://" + location.host + "/ws?token=" + token;
  }

  // ---- logging ---------------------------------------------------------
  function logLine(kind, text) {
    var line = document.createElement("div");
    line.className = "log-line log-" + (kind || "info");
    line.textContent = text;
    els["event-log"].appendChild(line);
    els["event-log"].scrollTop = els["event-log"].scrollHeight;
    return line;
  }

  // ---- connection state ------------------------------------------------
  function setConnState(s) {
    var el = els["conn-state"];
    el.className = "conn-state conn-" + s;
    if (s === "connecting") el.textContent = "연결 중…";
    else if (s === "open") el.textContent = "연결됨";
    else el.textContent = "재연결 대기 중…"; // retry
  }

  // ---- WebSocket lifecycle --------------------------------------------
  function connect() {
    state = "connecting";
    setConnState("connecting");
    try {
      ws = new WebSocket(wsUrl());
    } catch (e) {
      scheduleReconnect();
      return;
    }
    ws.onopen = onOpen;
    ws.onmessage = onMessage;
    ws.onerror = onError;
    ws.onclose = onClose;
  }

  function onOpen() {
    setConnState("open");
    state = "ready";
    logLine("info", "서버에 연결되었습니다.");
    // On (re)connect, re-request status and list.
    send({ type: "status" });
    send({ type: "list" });
    updateControls();
  }

  function onError() {
    // error → reconnect path; onclose will follow.
    setConnState("retry");
  }

  function onClose() {
    if (state !== "retry") {
      logLine("warn", "연결이 끊겼습니다. 재연결합니다…");
    }
    scheduleReconnect();
  }

  function scheduleReconnect() {
    state = "retry";
    setConnState("retry");
    updateControls();
    if (reconnectTimer) return;
    reconnectTimer = setTimeout(function () {
      reconnectTimer = null;
      connect();
    }, RECONNECT_BACKOFF_MS);
  }

  function send(obj) {
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(obj));
      return true;
    }
    return false;
  }

  // ---- inbound dispatch ------------------------------------------------
  function onMessage(ev) {
    var msg;
    try {
      msg = JSON.parse(ev.data);
    } catch (e) {
      logLine("warn", "잘못된 메시지를 받았습니다.");
      return;
    }
    switch (msg && msg.type) {
      case "progress":
        handleProgress(msg);
        break;
      case "code":
        handleCode(msg);
        break;
      case "applied":
        handleApplied(msg);
        break;
      case "error":
        handleError(msg);
        break;
      case "status":
        handleStatus(msg);
        break;
      case "list":
        handleList(msg);
        break;
      default:
        // unknown message type — log, never throw.
        logLine("warn", "알 수 없는 메시지 유형(unknown): " + (msg && msg.type));
        break;
    }
  }

  // ---- progress --------------------------------------------------------
  function clearSpinners() {
    Object.keys(spinnerLogs).forEach(function (stage) {
      var line = spinnerLogs[stage];
      if (line) line.classList.remove("log-spinner");
    });
    spinnerLogs = {};
  }

  function handleProgress(msg) {
    var stage = msg.stage;
    var label = PROGRESS_LABELS[stage] || stage || "진행 중";
    var detail = msg.detail ? " — " + msg.detail : "";
    // clear spinner on previously-active stages; this stage becomes active.
    clearSpinners();
    var line = logLine("progress", label + detail);
    line.classList.add("log-spinner");
    spinnerLogs[stage] = line;
    if (stage === "waiting_build") {
      state = "waiting";
      updateControls();
    }
  }

  // ---- code review -----------------------------------------------------
  function escapeHtml(s) {
    return String(s)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  function byteLength(s) {
    // UTF-8 byte length without allocating a Blob.
    return new TextEncoder().encode(s).length;
  }

  function handleCode(msg) {
    clearSpinners();
    state = "reviewing";
    currentCode = msg.code != null ? String(msg.code) : "";

    // language label
    els["code-lang"].textContent = msg.lang ? msg.lang : "";

    // preview (escaped, possibly truncated for display only)
    renderPreview(currentCode);

    // edit textarea seeded with full code — this is what Apply sends.
    els["edit-code"].value = currentCode;

    // diff (server-supplied unified diff; meaningful only for SET)
    renderDiff(newFileMode() ? "" : msg.diff || "");

    // diagnostics (advisory)
    renderDiagnostics(msg.diagnostics || []);

    selectTab("preview");
    updateControls();
  }

  function renderPreview(code) {
    var notice = els["preview-notice"];
    var display = code;
    if (byteLength(code) > PREVIEW_MAX_BYTES) {
      // Truncate the DISPLAY only; Apply still sends the full text.
      display = code.slice(0, PREVIEW_MAX_BYTES);
      notice.hidden = false;
      notice.textContent =
        "코드가 너무 커서 미리보기를 잘랐습니다. 적용 시에는 전체 코드가 전송됩니다.";
    } else {
      notice.hidden = true;
      notice.textContent = "";
    }
    var codeEl = els["preview-code"].querySelector("code");
    codeEl.innerHTML = escapeHtml(display);
  }

  function renderDiff(diff) {
    var view = els["diff-view"];
    view.innerHTML = "";
    if (!diff) {
      view.textContent = "변경 내용이 없습니다.";
      return;
    }
    var lines = diff.split("\n");
    lines.forEach(function (raw) {
      var div = document.createElement("div");
      div.className = "diff-line";
      if (raw.startsWith("+++") || raw.startsWith("---")) {
        div.classList.add("diff-file");
      } else if (raw.startsWith("@@")) {
        div.classList.add("diff-hunk");
      } else if (raw.startsWith("+")) {
        div.classList.add("diff-add");
      } else if (raw.startsWith("-")) {
        div.classList.add("diff-del");
      }
      div.textContent = raw;
      view.appendChild(div);
    });
  }

  function renderDiagnostics(diags) {
    var box = els["diagnostics"];
    var list = els["diagnostics-list"];
    list.innerHTML = "";
    if (!diags || !diags.length) {
      box.hidden = true;
      return;
    }
    diags.forEach(function (d) {
      var li = document.createElement("li");
      li.className = "diag-item";
      var msg = typeof d === "string" ? d : (d && (d.message || d.text)) || "";
      var sev = d && d.severity ? "[" + d.severity + "] " : "";
      var loc = d && d.line != null ? " (line " + d.line + ")" : "";
      li.textContent = sev + msg + loc;
      list.appendChild(li);
    });
    box.hidden = false; // advisory only — never blocks Apply
  }

  // ---- applied / error -------------------------------------------------
  function handleApplied(msg) {
    clearSpinners();
    state = "ready";
    logLine("ok", "적용 완료: " + (msg.target || ""));
    updateControls();
  }

  function handleError(msg) {
    clearSpinners();
    var text = msg.message || "알 수 없는 오류";
    logLine("error", "오류: " + text);
    // inline error near the NEWEPS input when in new-file mode.
    if (newFileMode()) {
      els["neweps-error"].textContent = text;
    }
    // error returns the flow to ready (per state machine).
    state = "ready";
    updateControls();
  }

  // ---- status / list ---------------------------------------------------
  function handleStatus(msg) {
    var name = msg.project || "";
    els["project-name"].textContent = name ? name : "프로젝트 없음";
    if (msg.compiling) {
      logLine("info", "에디터 빌드 중…");
    }
  }

  function handleList(msg) {
    var picker = els["target-picker"];
    picker.innerHTML = "";
    if (msg.error || !msg.files) {
      // No project open (or list error): placeholder, disable instruct.
      hasProject = false;
      var ph = document.createElement("option");
      ph.value = "";
      ph.disabled = true;
      ph.selected = true;
      ph.textContent = "프로젝트를 열어주세요";
      picker.appendChild(ph);
      updateControls();
      return;
    }
    hasProject = true;
    var files = msg.files;
    if (!files.length) {
      var empty = document.createElement("option");
      empty.value = "";
      empty.disabled = true;
      empty.selected = true;
      empty.textContent = "파일이 없습니다";
      picker.appendChild(empty);
    }
    files.forEach(function (f, i) {
      var opt = document.createElement("option");
      opt.value = f.path;
      var ftype = f.ftype != null ? f.ftype : "";
      opt.textContent = f.path + (ftype ? "  [" + ftype + "]" : "");
      if (f.settable === false) {
        // non-settable (GUI) types are disabled with a tooltip.
        opt.disabled = true;
        opt.title = READONLY_TITLE;
      } else if (i === 0) {
        opt.selected = true;
      }
      picker.appendChild(opt);
    });
    updateControls();
  }

  // ---- tabs ------------------------------------------------------------
  function selectTab(which) {
    var map = {
      preview: els["panel-preview"],
      diff: els["panel-diff"],
      edit: els["panel-edit"],
    };
    var tabs = {
      preview: els["tab-preview"],
      diff: els["tab-diff"],
      edit: els["tab-edit"],
    };
    Object.keys(map).forEach(function (k) {
      map[k].hidden = k !== which;
      tabs[k].classList.toggle("tab-active", k === which);
    });
  }

  // ---- new-file (NEWEPS) mode -----------------------------------------
  function newFileMode() {
    return els["new-file-toggle"].checked;
  }

  function applyNewFileMode() {
    var on = newFileMode();
    els["neweps-row"].hidden = !on;
    els["apply-set"].hidden = on;
    els["apply-neweps"].hidden = !on;
    // diff is meaningless for a new file.
    if (on) renderDiff("");
    updateControls();
  }

  // ---- NEWEPS filename validation -------------------------------------
  function validateNewEpsName(name) {
    var trimmed = (name || "").trim();
    if (trimmed.length === 0) {
      return { ok: false, reason: "파일 이름을 입력하세요." };
    }
    if (trimmed.indexOf("/") !== -1 || trimmed.indexOf("\\") !== -1) {
      return { ok: false, reason: "파일 이름에 경로 구분자(/ 또는 \\)를 쓸 수 없습니다." };
    }
    return { ok: true, name: trimmed };
  }

  // ---- control enable/disable -----------------------------------------
  function updateControls() {
    var connected = ws && ws.readyState === WebSocket.OPEN;
    var busy = state === "working" || state === "applying" || state === "waiting";
    var canInstruct = connected && hasProject && !busy;
    var canApply = connected && state === "reviewing" && !busy;

    els["send"].disabled = !canInstruct;
    els["instruction-input"].disabled = !connected || busy;
    els["apply-set"].disabled = !canApply;
    els["apply-neweps"].disabled = !canApply;
    els["cancel"].disabled = !(state === "reviewing");
    els["refresh-list"].disabled = !connected;
  }

  // ---- actions ---------------------------------------------------------
  function doInstruct() {
    if (els["send"].disabled) return;
    var instruction = els["instruction-input"].value.trim();
    if (!instruction) {
      logLine("warn", "지시 사항을 입력하세요.");
      return;
    }
    var target = els["target-picker"].value;
    var useContext = els["use-context"].checked;
    var ok = send({
      type: "instruct",
      instruction: instruction,
      target: target,
      useContext: useContext,
    });
    if (!ok) return;
    logLine("you", "지시: " + instruction);
    state = "working";
    updateControls();
  }

  function doApplySet() {
    if (els["apply-set"].disabled) return;
    var target = els["target-picker"].value;
    if (!target) {
      logLine("warn", "대상 파일을 선택하세요.");
      return;
    }
    var code = els["edit-code"].value; // textarea content is the source of truth
    send({ type: "apply", mode: "set", target: target, code: code });
    state = "applying";
    updateControls();
  }

  function doApplyNewEps() {
    if (els["apply-neweps"].disabled) return;
    var v = validateNewEpsName(els["neweps-name"].value);
    els["neweps-error"].textContent = "";
    if (!v.ok) {
      els["neweps-error"].textContent = v.reason;
      return;
    }
    var code = els["edit-code"].value;
    send({ type: "apply", mode: "neweps", target: v.name, code: code });
    state = "applying";
    updateControls();
  }

  function doCancel() {
    state = "ready";
    renderDiagnostics([]);
    updateControls();
    logLine("info", "검토를 취소했습니다.");
  }

  // ---- wiring ----------------------------------------------------------
  function wire() {
    els["tab-preview"].addEventListener("click", function () {
      selectTab("preview");
    });
    els["tab-diff"].addEventListener("click", function () {
      selectTab("diff");
    });
    els["tab-edit"].addEventListener("click", function () {
      selectTab("edit");
    });
    els["new-file-toggle"].addEventListener("change", applyNewFileMode);
    els["refresh-list"].addEventListener("click", function () {
      send({ type: "list" });
      send({ type: "status" });
    });
    els["send"].addEventListener("click", doInstruct);
    els["instruction-input"].addEventListener("keydown", function (e) {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        doInstruct();
      }
    });
    els["apply-set"].addEventListener("click", doApplySet);
    els["apply-neweps"].addEventListener("click", doApplyNewEps);
    els["cancel"].addEventListener("click", doCancel);
    els["diagnostics-dismiss"].addEventListener("click", function () {
      els["diagnostics"].hidden = true;
    });
    els["neweps-name"].addEventListener("input", function () {
      els["neweps-error"].textContent = "";
    });
  }

  // ---- boot ------------------------------------------------------------
  function boot() {
    cacheEls();
    wire();
    applyNewFileMode();
    updateControls();
    connect();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    boot();
  }
})();
