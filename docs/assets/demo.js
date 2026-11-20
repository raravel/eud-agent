// Interactive mockups for the landing page: four scripted scenarios that play
// on their own and pause at "gates" the visitor can click, the architecture
// flow diagram, and the install OS tabs. No dependencies.
(function () {
  "use strict";

  var REDUCED = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var CANCEL = { cancelled: true };
  var EN = /^en\b/i.test(document.documentElement.lang || "");
  function L(ko, en) {
    return EN ? en : ko;
  }

  // ---------------------------------------------------------------- helpers
  function h(tag, cls, html) {
    var node = document.createElement(tag);
    if (cls) node.className = cls;
    if (html != null) node.innerHTML = html;
    return node;
  }
  function esc(s) {
    return String(s).replace(/[&<>"]/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
    });
  }
  function icon(name, cls) {
    var paths = {
      check: '<path d="M20 6 9 17l-5-5"/>',
      tool: '<path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>',
      send: '<path d="m5 12 7-7 7 7"/><path d="M12 19V5"/>',
      file: '<path d="M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7Z"/><path d="M14 2v4a2 2 0 0 0 2 2h4"/>',
      hammer: '<path d="m15 12-8.5 8.5c-.83.83-2.17.83-3 0 0 0 0 0 0 0a2.12 2.12 0 0 1 0-3L12 9"/><path d="M17.64 15 22 10.64"/><path d="m20.91 11.7-1.25-1.25c-.6-.6-.93-1.4-.93-2.25v-.86L16.01 4.6a5.56 5.56 0 0 0-3.94-1.64H9l.92.82A6.18 6.18 0 0 1 12 8.4v1.56l2 2h2.47l2.26 1.91"/>',
      map: '<path d="M14.1 4.1a2 2 0 0 0-1.8 0L9.2 5.6a2 2 0 0 1-1.8 0L4.3 4.1A1 1 0 0 0 3 5v12.8a1 1 0 0 0 .6.9l4.8 2.4a2 2 0 0 0 1.8 0l3.1-1.5a2 2 0 0 1 1.8 0l3.1 1.5a1 1 0 0 0 1.4-.9V6.2a1 1 0 0 0-.6-.9z"/><path d="M15 5.8v15"/><path d="M9 3.2v15"/>',
      undo: '<path d="M3 7v6h6"/><path d="M21 17a9 9 0 0 0-9-9 9 9 0 0 0-6 2.3L3 13"/>',
      book: '<path d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H20v20H6.5a2.5 2.5 0 0 1 0-5H20"/>',
      git: '<circle cx="12" cy="12" r="3"/><line x1="3" x2="9" y1="12" y2="12"/><line x1="15" x2="21" y1="12" y2="12"/>',
      table: '<path d="M12 3v18"/><rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 9h18"/><path d="M3 15h18"/>',
      search: '<circle cx="11" cy="11" r="8"/><path d="m21 21-4.3-4.3"/>',
      spark: '<path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1"/>',
      shield: '<path d="M20 13c0 5-3.5 7.5-7.66 8.95a1 1 0 0 1-.67-.01C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.24-2.72a1.17 1.17 0 0 1 1.52 0C14.51 3.81 17 5 19 5a1 1 0 0 1 1 1z"/>',
      users: '<path d="M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2"/><circle cx="9" cy="7" r="4"/><path d="M22 21v-2a4 4 0 0 0-3-3.87"/><path d="M16 3.13a4 4 0 0 1 0 7.75"/>',
    };
    return (
      '<svg class="' + (cls || "h-3.5 w-3.5") + '" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">' +
      paths[name] + "</svg>"
    );
  }
  var SPINNER =
    '<svg class="h-3.5 w-3.5 animate-spin-slow text-app-primary" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" aria-hidden="true"><path d="M21 12a9 9 0 1 1-6.2-8.56" stroke-linecap="round"/></svg>';

  // ------------------------------------------------------------- the player
  var stage = document.getElementById("demo-stage");
  if (!stage) return;
  var chaptersEl = document.getElementById("demo-chapters");
  var captionEl = document.getElementById("demo-caption");
  var playBtn = document.getElementById("demo-play");
  var restartBtn = document.getElementById("demo-restart");
  var tabs = Array.prototype.slice.call(document.querySelectorAll("[data-demo-tab]"));

  var gen = 0; // bumps on every scenario (re)start; stale awaits reject
  var userPaused = REDUCED; // reduced motion: nothing moves until asked
  var visible = false;
  var current = null;

  function paused() {
    return userPaused || !visible || document.hidden;
  }
  function speed() {
    return REDUCED ? 0.2 : 1;
  }

  function sleep(ms) {
    var g = gen;
    return new Promise(function (resolve, reject) {
      var left = ms * speed();
      var last = performance.now();
      (function tick() {
        if (g !== gen) return reject(CANCEL);
        var now = performance.now();
        if (!paused()) left -= now - last;
        last = now;
        if (left <= 0) resolve();
        else setTimeout(tick, Math.min(40, left));
      })();
    });
  }

  // A gate waits for the visitor to press a real button; while the scenario
  // plays it presses itself after `auto` ms so the story keeps going.
  function gate(btn, auto) {
    var g = gen;
    btn.disabled = false;
    btn.classList.add("gate-pulse");
    return new Promise(function (resolve, reject) {
      var done = false;
      function finish(byUser) {
        if (done) return;
        done = true;
        btn.classList.remove("gate-pulse");
        btn.removeEventListener("click", onClick);
        resolve(byUser);
      }
      function onClick() {
        if (g !== gen) return;
        if (userPaused) setPaused(false);
        finish(true);
      }
      btn.addEventListener("click", onClick);
      sleep(auto || 2600).then(
        function () {
          finish(false);
        },
        function (e) {
          if (!done) {
            done = true;
            btn.classList.remove("gate-pulse");
            btn.removeEventListener("click", onClick);
            reject(e);
          }
        }
      );
    });
  }

  async function typeInto(el, text, cps) {
    el.textContent = "";
    var caret = h("span", "ml-px inline-block h-4 w-px translate-y-0.5 animate-blink bg-app-fg");
    for (var i = 0; i < text.length; i++) {
      el.textContent = text.slice(0, i + 1);
      el.appendChild(caret);
      await sleep(1000 / (cps || 38));
    }
    caret.remove();
  }

  function setPaused(p) {
    userPaused = p;
    playBtn.setAttribute("aria-label", p ? L("재생", "Play") : L("일시정지", "Pause"));
    playBtn.querySelector('[data-icon="pause"]').classList.toggle("hidden", p);
    playBtn.querySelector('[data-icon="play"]').classList.toggle("hidden", !p);
  }

  function renderChapters(list) {
    chaptersEl.innerHTML = "";
    list.forEach(function (label, i) {
      var li = h(
        "li",
        "min-w-0 flex-1",
        '<div class="h-1 overflow-hidden rounded-full bg-white/10"><div data-fill class="h-full w-0 rounded-full bg-emerald-400 transition-[width] duration-300"></div></div>' +
          '<p class="mt-1.5 truncate text-[11px] text-slate-500 sm:text-xs" data-label>' + esc(label) + "</p>"
      );
      li.dataset.index = i;
      chaptersEl.appendChild(li);
    });
  }
  function chapter(i, caption) {
    Array.prototype.forEach.call(chaptersEl.children, function (li, j) {
      li.querySelector("[data-fill]").style.width = j < i ? "100%" : j === i ? "55%" : "0";
      li.querySelector("[data-label]").className =
        "mt-1.5 truncate text-[11px] sm:text-xs " + (j === i ? "font-semibold text-emerald-300" : j < i ? "text-slate-400" : "text-slate-500");
      if (j === i) li.setAttribute("aria-current", "step");
      else li.removeAttribute("aria-current");
    });
    if (caption != null) captionEl.textContent = caption;
  }
  function finishChapters() {
    Array.prototype.forEach.call(chaptersEl.children, function (li) {
      li.querySelector("[data-fill]").style.width = "100%";
    });
  }

  var ORDER = ["eps", "map", "team", "dat"];
  var SCENARIOS = {};

  function start(id) {
    gen++;
    current = id;
    tabs.forEach(function (t) {
      var on = t.dataset.demoTab === id;
      t.setAttribute("aria-selected", on ? "true" : "false");
      t.tabIndex = on ? 0 : -1;
    });
    var sc = SCENARIOS[id];
    renderChapters(sc.chapters);
    stage.innerHTML = "";
    var ui = sc.build();
    stage.appendChild(ui.root);
    chapter(0, sc.intro);
    var g = gen;
    sc.run(ui).then(
      async function () {
        finishChapters();
        try {
          await sleep(9000);
        } catch (e) {
          return;
        }
        if (g === gen) start(ORDER[(ORDER.indexOf(id) + 1) % ORDER.length]);
      },
      function (e) {
        if (e !== CANCEL) throw e;
      }
    );
  }

  tabs.forEach(function (t, i) {
    t.addEventListener("click", function () {
      start(t.dataset.demoTab);
    });
    t.addEventListener("keydown", function (e) {
      var d = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
      if (!d) return;
      var next = tabs[(i + d + tabs.length) % tabs.length];
      next.focus();
      start(next.dataset.demoTab);
    });
  });
  playBtn.addEventListener("click", function () {
    setPaused(!userPaused);
  });
  restartBtn.addEventListener("click", function () {
    start(current);
  });
  new IntersectionObserver(
    function (entries) {
      visible = entries[0].isIntersecting;
    },
    { threshold: 0.25 }
  ).observe(stage);

  // --------------------------------------------------------- shared chrome
  function windowFrame(title, badge) {
    var root = h("div", "mock-window text-[13px] text-app-fg");
    root.innerHTML =
      '<div class="flex h-9 items-center gap-2 border-b border-app-line bg-black/20 px-3">' +
      '<span class="grid h-4 w-4 place-items-center rounded bg-sky-400/20 text-[9px] font-bold text-sky-300">E</span>' +
      '<span class="truncate text-xs text-app-dim">' + esc(title) + "</span>" +
      (badge ? '<span class="ml-1 hidden rounded bg-white/5 px-1.5 py-0.5 text-[10px] text-app-dim sm:inline">' + esc(badge) + "</span>" : "") +
      '<span class="ml-auto flex gap-3 text-app-dim/60" aria-hidden="true"><span>—</span><span>▢</span><span>✕</span></span></div>';
    return root;
  }

  function appHeader(project, extra) {
    return (
      '<div class="flex flex-wrap items-center gap-2 border-b border-app-line px-3 py-2">' +
      '<span class="font-semibold">' + esc(project) + "</span>" +
      '<span class="inline-flex items-center gap-1.5 rounded-full border border-app-line px-2 py-0.5 text-[11px] text-app-dim"><span class="h-1.5 w-1.5 rounded-full bg-app-primary"></span>' + L("RAG: 준비됨", "RAG: ready") + '</span>' +
      '<span class="hidden items-center gap-1.5 rounded-full border border-app-line px-2 py-0.5 text-[11px] text-app-dim sm:inline-flex"><span class="h-1.5 w-1.5 rounded-full bg-sky-400"></span>Codex</span>' +
      '<span class="ml-auto flex gap-1.5">' + (extra || "") +
      '<span class="hidden rounded-md border border-app-line px-2 py-1 text-[11px] text-app-dim md:inline">' + L("프로젝트 빌드", "Build") + '</span>' +
      '<span class="hidden rounded-md border border-app-line px-2 py-1 text-[11px] text-app-dim md:inline">' + L("맵 에이전트", "Map Agent") + '</span>' +
      '<span class="hidden rounded-md border border-app-line px-2 py-1 text-[11px] text-app-dim lg:inline">' + L("프로젝트 전환", "Switch project") + '</span></span></div>'
    );
  }

  // Chat column: message log + composer with a real send button.
  function chatPanel(title) {
    var col = h("div", "flex min-h-0 flex-col bg-app-card/60");
    col.innerHTML =
      '<div class="flex items-center gap-2 border-b border-app-line px-3 py-2 text-xs text-app-dim">' + icon("spark", "h-3.5 w-3.5 text-app-primary") + esc(title) + "</div>" +
      '<div data-log class="mock-scroll flex min-h-0 flex-1 flex-col gap-2.5 p-3"></div>' +
      '<div class="border-t border-app-line p-2.5"><div class="flex items-end gap-2 rounded-lg border border-app-line bg-app-bg px-2.5 py-2">' +
      '<p data-input class="min-h-9 flex-1 text-[13px] leading-relaxed text-app-fg"><span class="text-app-dim/70">' + L("요청을 입력하세요 · @로 멘션", "Describe a change · @ to mention") + '</span></p>' +
      '<button type="button" data-send disabled class="grid h-8 w-8 shrink-0 place-items-center rounded-md bg-app-primary text-app-primary-fg disabled:opacity-40" aria-label="' + L("요청 보내기", "Send request") + '">' + icon("send", "h-4 w-4") + "</button></div></div>";
    var log = col.querySelector("[data-log]");
    var input = col.querySelector("[data-input]");
    var send = col.querySelector("[data-send]");
    // Keep the newest line in view while a reply types itself out.
    new MutationObserver(function () {
      log.scrollTop = log.scrollHeight;
    }).observe(log, { childList: true, subtree: true, characterData: true });

    function push(node) {
      log.appendChild(node);
      log.scrollTop = log.scrollHeight;
      return node;
    }
    return {
      el: col,
      log: log,
      async ask(text, auto) {
        await typeInto(input, text);
        await gate(send, auto || 1800);
        send.disabled = true;
        input.innerHTML = '<span class="text-app-dim/70">' + L("요청을 입력하세요 · @로 멘션", "Describe a change · @ to mention") + '</span>';
        push(h("div", "ml-auto max-w-[88%] animate-rise rounded-lg rounded-br-sm bg-app-muted px-3 py-2 leading-relaxed", esc(text).replace(/@(\S+)/g, '<span class="rounded bg-emerald-400/15 px-1 text-emerald-200">@$1</span>')));
      },
      async say(text) {
        var p = push(h("p", "animate-rise leading-relaxed text-app-fg/90"));
        await typeInto(p, text, 70);
        return p;
      },
      note(html) {
        return push(h("div", "animate-rise rounded-md border border-app-line bg-black/20 px-2.5 py-1.5 text-xs text-app-dim", html));
      },
      // A tool call row: spinner while running, then a check and a result line.
      async tool(name, args, result, ms) {
        var row = push(
          h(
            "div",
            "animate-rise rounded-md border border-app-line bg-app-bg/70 px-2.5 py-1.5",
            '<div class="flex items-center gap-2"><span data-st>' + SPINNER + '</span><span class="font-mono text-[12px] text-sky-200">' + esc(name) + '</span><span class="truncate font-mono text-[11px] text-app-dim">' + esc(args || "") + "</span></div>"
          )
        );
        await sleep(ms || 900);
        row.querySelector("[data-st]").innerHTML = icon("check", "h-3.5 w-3.5 text-app-primary");
        if (result) {
          row.appendChild(h("p", "mt-1 pl-5.5 text-[11px] leading-relaxed text-app-dim", result));
          log.scrollTop = log.scrollHeight;
        }
        return row;
      },
      push: push,
    };
  }

  // Code editor pane with tabs.
  function editorPane() {
    var el = h("div", "flex min-h-0 flex-col");
    el.innerHTML =
      '<div data-tabs class="flex h-9 items-end gap-px overflow-hidden border-b border-app-line bg-black/20 px-2"></div>' +
      '<div data-body class="mock-scroll min-h-0 flex-1 py-2 font-mono text-[12px] leading-[1.7]"></div>';
    var tabsEl = el.querySelector("[data-tabs]");
    var body = el.querySelector("[data-body]");
    return {
      el: el,
      body: body,
      tabs: function (names, active) {
        tabsEl.innerHTML = names
          .map(function (n) {
            var on = n === active;
            return '<span class="flex items-center gap-1.5 rounded-t-md px-3 py-1.5 text-[12px] ' + (on ? "bg-app-bg text-app-fg" : "text-app-dim") + '">' + icon("file", "h-3 w-3") + esc(n) + "</span>";
          })
          .join("");
      },
      show: function (lines) {
        body.innerHTML = "";
        lines.forEach(function (l, i) {
          body.appendChild(codeLine(l, i));
        });
      },
      async reveal(lines, from) {
        for (var i = 0; i < lines.length; i++) {
          var node = codeLine(lines[i], i);
          if (i >= (from || 0) && lines[i][0]) {
            node.classList.add("animate-rise");
            body.appendChild(node);
            await sleep(110);
          } else body.appendChild(node);
        }
      },
      overlay: function (node) {
        el.style.position = "relative";
        el.appendChild(node);
      },
    };
  }
  function codeLine(l, i) {
    // l = [kind, html] where kind is "", "add" or "del"
    var row = h("div", "code-line " + (l[0] || ""));
    row.innerHTML = '<span class="ln">' + (i + 1) + '</span><span class="mark">' + (l[0] === "add" ? "+" : l[0] === "del" ? "−" : "") + "</span><span>" + (l[1] || " ") + "</span>";
    return row;
  }
  function K(s) {
    return '<span class="tok-k">' + s + "</span>";
  }
  function S(s) {
    return '<span class="tok-s">' + esc(s) + "</span>";
  }
  function N(s) {
    return '<span class="tok-n">' + s + "</span>";
  }
  function F(s) {
    return '<span class="tok-f">' + s + "</span>";
  }
  function C(s) {
    return '<span class="tok-c">' + esc(s) + "</span>";
  }

  function sideTabs(active) {
    var items = [
      ["files", "file", L("파일", "Files")],
      ["rag", "book", L("참고 문서", "Docs")],
      ["dat", "table", L("DAT 위키", "DAT wiki")],
      ["git", "git", L("변경 기록", "History")],
    ];
    return (
      '<div class="grid grid-cols-4 border-b border-app-line">' +
      items
        .map(function (it) {
          var on = it[0] === active;
          return '<span data-side="' + it[0] + '" class="flex flex-col items-center gap-0.5 py-1.5 text-[10px] ' + (on ? "text-app-primary" : "text-app-dim") + '">' + icon(it[1], "h-3.5 w-3.5") + it[2] + "</span>";
        })
        .join("") +
      "</div>"
    );
  }
  function setSideTab(root, active) {
    root.querySelectorAll("[data-side]").forEach(function (s) {
      var on = s.dataset.side === active;
      s.classList.toggle("text-app-primary", on);
      s.classList.toggle("text-app-dim", !on);
    });
  }

  function fileTree(files) {
    return files
      .map(function (f) {
        var badge = f.s ? '<span class="ml-auto rounded px-1 text-[10px] font-semibold ' + (f.s === "A" ? "bg-emerald-400/15 text-emerald-300" : "bg-amber-400/15 text-amber-300") + '">' + f.s + "</span>" : "";
        return '<div class="flex items-center gap-1.5 rounded px-2 py-1 ' + (f.on ? "bg-white/5 text-app-fg" : "text-app-dim") + '" style="padding-left:' + (0.5 + (f.d || 0) * 0.75) + 'rem">' + (f.dir ? "▾ " : icon("file", "h-3 w-3 shrink-0")) + '<span class="truncate">' + esc(f.n) + "</span>" + badge + "</div>";
      })
      .join("");
  }

  // Main window: header, sidebar, center, chat.
  function mainWindow(opts) {
    var root = windowFrame("eud-agent — " + opts.project, opts.badge);
    var body = h("div", "");
    body.innerHTML =
      appHeader(opts.project) +
      '<div class="grid grid-cols-[minmax(0,1fr)] lg:h-[540px] lg:grid-cols-[13rem_minmax(0,1fr)_21rem] md:grid-cols-[12rem_minmax(0,1fr)]">' +
      '<aside data-side-root class="hidden min-h-0 flex-col border-r border-app-line md:flex">' + sideTabs(opts.side || "files") + '<div data-side-body class="mock-scroll min-h-0 flex-1 p-1.5 text-[12px]"></div></aside>' +
      '<div data-center class="flex h-72 min-h-0 flex-col md:h-[420px] lg:h-auto"></div>' +
      '<div data-chat class="flex h-[380px] min-h-0 flex-col border-t border-app-line md:col-span-2 lg:col-span-1 lg:h-auto lg:border-l lg:border-t-0"></div>' +
      "</div>";
    root.appendChild(body);
    var chat = chatPanel(opts.chatTitle || L("세션 · 새 대화", "Session · New chat"));
    chat.el.classList.add("flex-1");
    body.querySelector("[data-chat]").appendChild(chat.el);
    return {
      root: root,
      side: body.querySelector("[data-side-body]"),
      sideRoot: body.querySelector("[data-side-root]"),
      center: body.querySelector("[data-center]"),
      chat: chat,
    };
  }

  function dialog(host, html) {
    var wrap = h("div", "absolute inset-0 z-10 grid place-items-center bg-black/55 p-4 animate-rise");
    wrap.innerHTML = '<div class="w-full max-w-sm rounded-xl border border-app-line bg-app-card p-4 shadow-2xl">' + html + "</div>";
    host.style.position = "relative";
    host.appendChild(wrap);
    return wrap;
  }

  // ------------------------------------------------------------ map canvas
  var W = 40,
    Hh = 28,
    TS = 16;
  var COLORS = {
    dirt: ["#7a6044", "#735a40", "#80654a"],
    grass: ["#556b36", "#4e6431", "#5b723b"],
    water: ["#27476a", "#2a4d73", "#244264"],
    high: ["#9b7e57", "#94784f", "#a2855e"],
    cliff: ["#4b3a28", "#433424", "#52402c"],
    stone: ["#5d5b66", "#57555f", "#63616c"],
  };
  function rng(seed) {
    return function () {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed / 0x7fffffff;
    };
  }
  function baseMap() {
    var t = [];
    for (var y = 0; y < Hh; y++) {
      for (var x = 0; x < W; x++) {
        var riverY = -0.5 * x + 22;
        var kind = "dirt";
        var n = Math.sin(x * 0.35) + Math.cos(y * 0.42) + Math.sin((x + y) * 0.2);
        if (n > 1.3) kind = "grass";
        if (Math.abs(y - riverY) < 1.6 && !(x >= 17 && x <= 19)) kind = "water";
        if (x >= 33 && y >= 21) kind = x === 33 || y === 21 ? "cliff" : "high";
        t.push(kind);
      }
    }
    return {
      tiles: t,
      doodads: [
        [3, 12], [9, 15], [15, 4], [26, 3], [37, 9], [12, 26], [22, 26], [30, 13],
      ].map(function (d) {
        return { x: d[0], y: d[1], k: "tree" };
      }),
      units: [
        { x: 4, y: 25, c: "#3b82f6", l: "P1" },
        { x: 35, y: 14, c: "#ef4444", l: "P2" },
      ],
    };
  }
  function cloneMap(m) {
    return { tiles: m.tiles.slice(), doodads: m.doodads.slice(), units: m.units.slice() };
  }
  function drawMap(canvas, m, ov) {
    var ctx = canvas.getContext("2d");
    var r = rng(7);
    for (var y = 0; y < Hh; y++) {
      for (var x = 0; x < W; x++) {
        var pal = COLORS[m.tiles[y * W + x]];
        ctx.fillStyle = pal[Math.floor(r() * pal.length)];
        ctx.fillRect(x * TS, y * TS, TS, TS);
        // subtle speckle
        ctx.fillStyle = "rgba(0,0,0,0.12)";
        ctx.fillRect(x * TS + Math.floor(r() * 12), y * TS + Math.floor(r() * 12), 2, 2);
      }
    }
    // cliff shading on the south edge of high ground
    for (y = 0; y < Hh - 1; y++) {
      for (x = 0; x < W; x++) {
        var a = m.tiles[y * W + x], b = m.tiles[(y + 1) * W + x];
        if ((a === "high" || a === "cliff") && b !== "high" && b !== "cliff") {
          ctx.fillStyle = "rgba(20,12,4,0.55)";
          ctx.fillRect(x * TS, (y + 1) * TS - 4, TS, 4);
        }
      }
    }
    m.doodads.forEach(function (d) {
      var cx = d.x * TS + TS / 2, cy = d.y * TS + TS / 2;
      if (d.k === "torch") {
        ctx.fillStyle = "#f59e0b";
        ctx.beginPath();
        ctx.arc(cx, cy, 4, 0, Math.PI * 2);
        ctx.fill();
        ctx.fillStyle = "rgba(245,158,11,0.25)";
        ctx.beginPath();
        ctx.arc(cx, cy, 9, 0, Math.PI * 2);
        ctx.fill();
        return;
      }
      ctx.fillStyle = "rgba(0,0,0,0.35)";
      ctx.beginPath();
      ctx.ellipse(cx + 2, cy + 5, 7, 3, 0, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = "#2c4a22";
      ctx.beginPath();
      ctx.arc(cx, cy, 7, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = "#3f6630";
      ctx.beginPath();
      ctx.arc(cx - 2, cy - 2, 4, 0, Math.PI * 2);
      ctx.fill();
    });
    m.units.forEach(function (u) {
      ctx.fillStyle = u.c;
      ctx.fillRect(u.x * TS - 6, u.y * TS - 6, TS + 12, TS + 12);
      ctx.fillStyle = "rgba(255,255,255,0.9)";
      ctx.font = "bold 12px sans-serif";
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(u.l, u.x * TS + TS / 2, u.y * TS + TS / 2 + 1);
    });
    ov = ov || {};
    if (ov.diff) {
      ctx.fillStyle = "rgba(52,211,153,0.22)";
      ov.diff.forEach(function (i) {
        ctx.fillRect((i % W) * TS, Math.floor(i / W) * TS, TS, TS);
      });
    }
    (ov.rects || []).forEach(function (rc) {
      ctx.save();
      ctx.strokeStyle = rc.color;
      ctx.lineWidth = 2;
      ctx.setLineDash(rc.dash ? [6, 4] : []);
      ctx.fillStyle = rc.fill || "transparent";
      ctx.fillRect(rc.x * TS, rc.y * TS, rc.w * TS, rc.h * TS);
      ctx.strokeRect(rc.x * TS + 1, rc.y * TS + 1, rc.w * TS - 2, rc.h * TS - 2);
      if (rc.label) {
        ctx.font = "600 12px 'Pretendard Variable', sans-serif";
        var tw = ctx.measureText(rc.label).width + 10;
        ctx.fillStyle = rc.color;
        ctx.fillRect(rc.x * TS, rc.y * TS - 18, tw, 17);
        ctx.fillStyle = "#06150f";
        ctx.textAlign = "left";
        ctx.textBaseline = "middle";
        ctx.fillText(rc.label, rc.x * TS + 5, rc.y * TS - 9);
      }
      ctx.restore();
    });
  }
  function mapCanvas() {
    var c = document.createElement("canvas");
    c.width = W * TS;
    c.height = Hh * TS;
    c.className = "block h-auto w-full rounded-md [image-rendering:pixelated]";
    c.setAttribute("role", "img");
    return c;
  }
  // Paint a list of [index, kind] changes progressively.
  async function paint(canvas, m, changes, ov, batch) {
    for (var i = 0; i < changes.length; i += batch || 8) {
      changes.slice(i, i + (batch || 8)).forEach(function (c) {
        m.tiles[c[0]] = c[1];
      });
      drawMap(canvas, m, ov);
      await sleep(40);
    }
  }
  function hillChanges(x0, y0, w, hgt) {
    var out = [];
    for (var y = y0; y < y0 + hgt; y++)
      for (var x = x0; x < x0 + w; x++) {
        var edge = x === x0 || y === y0 || x === x0 + w - 1 || y === y0 + hgt - 1;
        out.push([y * W + x, edge ? "cliff" : "high"]);
      }
    return out;
  }

  // Map window: toolbar, palette, canvas, chat.
  function mapWindow(opts) {
    var root = windowFrame(L("맵 에이전트 — ", "Map Agent — ") + opts.map, opts.badge);
    var body = h("div", "");
    body.innerHTML =
      '<div class="flex flex-wrap items-center gap-1.5 border-b border-app-line px-3 py-2">' +
      '<span class="inline-flex rounded-md border border-app-line p-0.5 text-[11px]"><span data-view="orig" class="rounded px-2 py-0.5 bg-white/10">' + L("원본", "Original") + '</span><span data-view="cand" class="rounded px-2 py-0.5 text-app-dim">' + L("후보", "Candidate") + '</span></span>' +
      '<span data-rev class="rounded-full border border-app-line px-2 py-0.5 text-[11px] text-app-dim">' + L("r0 · 기준", "r0 · base") + '</span>' +
      '<span class="ml-auto flex gap-1.5">' +
      '<span class="hidden rounded-md border border-app-line px-2 py-1 text-[11px] text-app-dim md:inline">' + L("맵 속성", "Properties") + '</span>' +
      '<span class="hidden rounded-md border border-app-line px-2 py-1 text-[11px] text-app-dim md:inline">' + L("이미지 내보내기", "Export image") + '</span>' +
      '<button type="button" data-undo disabled class="inline-flex items-center gap-1 rounded-md border border-app-line px-2.5 py-1 text-[12px] disabled:opacity-40">' + icon("undo", "h-3 w-3") + "Undo</button>" +
      '<button type="button" data-apply disabled class="rounded-md bg-app-primary px-3 py-1 text-[12px] font-semibold text-app-primary-fg disabled:opacity-40">Apply</button></span></div>' +
      '<div class="grid grid-cols-[minmax(0,1fr)] lg:h-[520px] ' + (opts.chat ? "lg:grid-cols-[11rem_minmax(0,1fr)_20rem]" : "lg:grid-cols-[minmax(0,1fr)]") + ' md:grid-cols-[11rem_minmax(0,1fr)]">' +
      '<aside class="hidden min-h-0 flex-col border-r border-app-line p-2 md:flex ' + (opts.chat ? "" : "lg:hidden") + '"><p class="px-1 pb-1.5 text-[11px] font-semibold text-app-dim">' + L("선택 영역", "Selections") + '</p><div data-palette class="flex flex-col gap-1 text-[12px]"><p class="px-1 text-[11px] text-app-dim/70">' + L("저장된 영역 없음", "No saved areas") + '</p></div></aside>' +
      '<div data-canvas-host class="relative grid min-h-0 place-items-center bg-black/30 p-2 sm:p-3"></div>' +
      (opts.chat ? '<div data-chat class="flex h-[360px] min-h-0 flex-col border-t border-app-line md:col-span-2 lg:col-span-1 lg:h-auto lg:border-l lg:border-t-0"></div>' : "") +
      "</div>";
    root.appendChild(body);
    var canvas = mapCanvas();
    canvas.setAttribute("aria-label", L("맵 캔버스 목업", "Map canvas mockup"));
    body.querySelector("[data-canvas-host]").appendChild(canvas);
    var chat = null;
    if (opts.chat) {
      chat = chatPanel("Map Agent");
      chat.el.classList.add("flex-1");
      body.querySelector("[data-chat]").appendChild(chat.el);
    }
    return {
      root: root,
      canvas: canvas,
      host: body.querySelector("[data-canvas-host]"),
      palette: body.querySelector("[data-palette]"),
      apply: body.querySelector("[data-apply]"),
      undo: body.querySelector("[data-undo]"),
      rev: body.querySelector("[data-rev]"),
      view: function (which) {
        body.querySelectorAll("[data-view]").forEach(function (v) {
          var on = v.dataset.view === which;
          v.classList.toggle("bg-white/10", on);
          v.classList.toggle("text-app-dim", !on);
        });
      },
      chat: chat,
    };
  }

  function checklist(host, items) {
    var box = h("div", "absolute bottom-3 left-3 right-3 z-10 rounded-lg border border-app-line bg-app-card/95 p-3 text-[12px] shadow-xl animate-rise sm:left-auto sm:w-64");
    box.innerHTML = '<p class="mb-2 flex items-center gap-1.5 font-semibold">' + icon("shield", "h-3.5 w-3.5 text-app-primary") + "MapSafe Apply</p>" +
      items.map(function (t) {
        return '<p class="flex items-center gap-2 py-0.5 text-app-dim" data-item><span data-st class="grid h-3.5 w-3.5 place-items-center"><span class="h-1.5 w-1.5 rounded-full bg-app-dim/40"></span></span>' + esc(t) + "</p>";
      }).join("");
    host.appendChild(box);
    return {
      box: box,
      async run(ms) {
        var rows = box.querySelectorAll("[data-item]");
        for (var i = 0; i < rows.length; i++) {
          rows[i].querySelector("[data-st]").innerHTML = SPINNER;
          await sleep(ms || 420);
          rows[i].querySelector("[data-st]").innerHTML = icon("check", "h-3.5 w-3.5 text-app-primary");
          rows[i].classList.remove("text-app-dim");
        }
      },
    };
  }

  // =============================================================== 01 EPS
  var MAIN_ORIG = [
    ["", K("import") + " py_eudplib;"],
    ["", ""],
    ["", K("function") + " " + F("onPluginStart") + "() {"],
    ["", "    " + C(L("// 맵 초기화", "// map setup"))],
    ["", "}"],
    ["", ""],
    ["", K("function") + " " + F("beforeTriggerExec") + "() {"],
    ["", "}"],
  ];
  var MAIN_NEW = [
    ["", K("import") + " py_eudplib;"],
    ["add", K("import") + " wave;"],
    ["", ""],
    ["", K("function") + " " + F("onPluginStart") + "() {"],
    ["", "    " + C(L("// 맵 초기화", "// map setup"))],
    ["", "}"],
    ["", ""],
    ["", K("function") + " " + F("beforeTriggerExec") + "() {"],
    ["add", "    wave." + F("tick") + "();"],
    ["", "}"],
  ];
  var WAVE = [
    ["add", K("var") + " timer = " + N("0") + ";"],
    ["add", K("var") + " started = " + N("0") + ";"],
    ["add", ""],
    ["add", K("function") + " " + F("tick") + "() {"],
    ["add", "    " + K("if") + " (started) " + K("return") + ";"],
    ["add", "    timer += " + N("1") + ";"],
    ["add", "    " + K("if") + " (timer == " + N("24") + " * " + N("10") + ") {"],
    ["add", "        started = " + N("1") + ";"],
    ["add", "        " + K("foreach") + " (p : " + F("EUDLoopPlayer") + "(" + S('"Human"') + ")) {"],
    ["add", "            " + F("setcurpl") + "(p);"],
    ["add", "            " + F("DisplayText") + "(" + S(L('"\\x04웨이브 1 시작!"', '"\\x04Wave 1 begins!"')) + ");"],
    ["add", "        }"],
    ["add", "        " + F("CreateUnit") + "(" + N("12") + ", " + S('"Zerg Zergling"') + ", " + S('"spawn"') + ", P8);"],
    ["add", "    }"],
    ["add", "}"],
  ];

  SCENARIOS.eps = {
    chapters: L(["요청", "읽기·검색", "수정", "빌드", "기록·되돌리기"], ["Request", "Read & search", "Edit", "Build", "Commit & revert"]),
    intro: L("메인 창의 채팅에 원하는 동작을 평소 말투로 적습니다.", "Describe what you want in the main window’s chat, in plain words."),
    build: function () {
      var ui = mainWindow({ project: "wave-defense", badge: L("EPS 세션", "EPS session"), side: "files", chatTitle: L("세션 · 웨이브 추가", "Session · Add a wave") });
      ui.editor = editorPane();
      ui.center.appendChild(ui.editor.el);
      ui.editor.tabs(["main.eps"], "main.eps");
      ui.editor.show(MAIN_ORIG);
      ui.files = [
        { n: "src", dir: 1 },
        { n: "main.eps", d: 1, on: 1 },
        { n: "units.eps", d: 1 },
        { n: "dat", dir: 1 },
        { n: "standard.json", d: 1 },
        { n: "project.eap" },
      ];
      ui.side.innerHTML = fileTree(ui.files);
      return ui;
    },
    run: async function (ui) {
      var chat = ui.chat;
      await sleep(600);
      await chat.ask(L("게임 시작 10초 뒤에 모든 플레이어에게 '웨이브 1 시작!'을 띄우고 spawn 로케이션에 저글링 12마리를 P8로 만들어줘", "10 seconds into the game, show 'Wave 1 begins!' to every player and spawn 12 Zerglings for P8 at the spawn location"));

      chapter(1, L("에이전트가 프로젝트 파일과 맵을 직접 읽고, 필요한 문법은 내장된 참고 문서에서 찾습니다.", "The agent reads the project files and the map itself, and looks up syntax in the bundled reference docs."));
      await chat.tool("fs_read", "src/main.eps", L("8줄 · MainFile", "8 lines · MainFile"));
      await chat.tool("map_info", "locations", L('로케이션 <span class="text-app-fg">spawn</span> (id 3) 확인', 'Found location <span class="text-app-fg">spawn</span> (id 3)'));
      await chat.tool("search_docs", "CreateUnit DisplayText EUDLoopPlayer", L("참고 문서 4건 · epScript 레퍼런스", "4 docs · epScript reference"));

      chapter(2, L("새 파일을 만들고 기존 파일은 필요한 줄만 고칩니다. 바뀐 줄이 에디터에 바로 표시됩니다.", "It creates new files and edits only the lines it needs in existing ones. Changes show up in the editor right away."));
      await chat.say(L("src/wave.eps에 웨이브 로직을 만들고 main.eps에서 매 트리거마다 호출하겠습니다.", "I’ll put the wave logic in src/wave.eps and call it from main.eps on every trigger pass."));
      var row = chat.tool("fs_write", "src/wave.eps", L("15줄 작성", "15 lines written"), 600);
      ui.editor.tabs(["main.eps", "wave.eps"], "wave.eps");
      ui.editor.body.innerHTML = "";
      ui.files.splice(3, 0, { n: "wave.eps", d: 1, s: "A" });
      ui.files[1].on = 0;
      ui.files[3].on = 1;
      ui.side.innerHTML = fileTree(ui.files);
      await ui.editor.reveal(WAVE);
      await row;
      await chat.tool("fs_edit", "src/main.eps  +2", L("import 1줄, 호출 1줄 추가", "added 1 import, 1 call"), 700);
      ui.editor.tabs(["main.eps", "wave.eps"], "main.eps");
      ui.files[1].s = "M";
      ui.files[1].on = 1;
      ui.files[3].on = 0;
      ui.side.innerHTML = fileTree(ui.files);
      ui.editor.show(MAIN_NEW);
      await sleep(900);

      chapter(3, L("수정이 끝나면 에이전트가 직접 euddraft로 빌드합니다. 새 출력 맵이 만들어져야 성공입니다.", "When the edits are done, the agent builds with euddraft itself. A build only passes if a fresh output map appears."));
      var build = chat.tool("build_run", "", L('<span class="text-app-primary">성공</span> · 오류 0 · 경고 0 · 2.4초', '<span class="text-app-primary">Success</span> · 0 errors · 0 warnings · 2.4s'), 2600);
      await build;
      var dlg = dialog(
        ui.center,
        '<p class="flex items-center gap-2 font-semibold">' + icon("hammer", "h-4 w-4 text-app-primary") + L("프로젝트 빌드", "Build project") + ' <span class="rounded-full bg-emerald-400/15 px-2 py-0.5 text-[11px] text-emerald-300">' + L("성공", "Success") + '</span></p>' +
          '<p class="mt-1 text-xs text-app-dim">' + L("euddraft가 출력 맵을 새로 만들었습니다.", "euddraft wrote a fresh output map.") + '</p>' +
          '<dl class="mt-3 space-y-1.5 text-[12px]">' +
          '<div class="flex justify-between gap-3"><dt class="text-app-dim">' + L("출력 맵", "Output map") + '</dt><dd class="truncate font-mono">build/[EUD]wave-defense.scx</dd></div>' +
          '<div class="flex justify-between gap-3"><dt class="text-app-dim">' + L("스타크래프트 복사본", "StarCraft copy") + '</dt><dd class="truncate font-mono">Maps/eud-agent/</dd></div>' +
          '<div class="flex justify-between gap-3"><dt class="text-app-dim">' + L("euddraft 종료 코드", "euddraft exit code") + '</dt><dd class="font-mono">0</dd></div>' +
          '<div class="flex justify-between gap-3"><dt class="text-app-dim">' + L("전체 로그", "Full log") + '</dt><dd class="truncate font-mono">build/euddraft/build.log</dd></div></dl>'
      );
      await sleep(2600);
      dlg.remove();

      chapter(4, L("요청 하나가 끝나면 git 커밋으로 남습니다. 마음에 들지 않으면 변경 기록에서 되돌립니다.", "Each finished request becomes a git commit. Don’t like it? Revert it from History."));
      await chat.say(L("완료했습니다. 게임 시작 240틱 뒤 메시지를 띄우고 spawn에 저글링 12기를 생성합니다. 빌드도 성공했습니다.", "Done. 240 ticks into the game it shows the message and spawns 12 Zerglings at spawn. The build passed too."));
      setSideTab(ui.sideRoot, "git");
      ui.side.innerHTML =
        '<p class="px-2 pb-1 pt-1 text-[11px] font-semibold text-app-dim">' + L("변경 기록", "History") + '</p>' +
        '<div data-commit class="animate-rise rounded-md border border-app-line bg-white/[0.03] p-2">' +
        '<p class="font-medium leading-snug">' + L("웨이브 1 메시지와 저글링 스폰 추가", "Add wave 1 message and Zergling spawn") + '</p>' +
        '<p class="mt-0.5 text-[10px] text-app-dim">' + L("방금 · 세션 a3f · 요청 7", "just now · session a3f · request 7") + '</p>' +
        '<p class="mt-1.5 font-mono text-[11px]"><span class="text-emerald-300">A</span> src/wave.eps<br><span class="text-amber-300">M</span> src/main.eps</p>' +
        '<button type="button" data-revert class="mt-2 inline-flex w-full items-center justify-center gap-1 rounded-md border border-app-line py-1 text-[11px] hover:bg-white/5">' + icon("undo", "h-3 w-3") + L("되돌리기", "Revert") + "</button></div>";
      if (!ui.sideRoot.offsetParent) chat.note(L('변경 기록 · <span class="text-app-fg">웨이브 1 메시지와 저글링 스폰 추가</span> — 사이드바의 변경 기록 탭에서 되돌릴 수 있습니다.', 'History · <span class="text-app-fg">Add wave 1 message and Zergling spawn</span> — revert it from the History tab in the sidebar.'));
      var revertBtn = ui.side.querySelector("[data-revert]");
      if (!ui.sideRoot.offsetParent) {
        await sleep(2500);
        return;
      }
      await gate(revertBtn, 4200);
      var confirm = dialog(
        ui.center,
        '<p class="font-semibold">' + L("이 변경을 되돌릴까요?", "Revert this change?") + '</p><p class="mt-1.5 text-xs leading-relaxed text-app-dim">' + L("커밋을 지우지 않고, 반대 변경을 새 커밋으로 기록합니다.", "The commit stays; the reverse change is recorded as a new commit.") + '</p>' +
          '<div class="mt-3 flex justify-end gap-2"><span class="rounded-md border border-app-line px-3 py-1 text-[12px] text-app-dim">' + L("취소", "Cancel") + '</span><button type="button" data-ok class="rounded-md bg-app-primary px-3 py-1 text-[12px] font-semibold text-app-primary-fg">' + L("되돌리기", "Revert") + '</button></div>'
      );
      await gate(confirm.querySelector("[data-ok]"), 1800);
      confirm.remove();
      ui.editor.tabs(["main.eps"], "main.eps");
      ui.editor.show(MAIN_ORIG);
      ui.files = ui.files.filter(function (f) {
        return f.n !== "wave.eps";
      });
      ui.files[1].s = "";
      ui.side.innerHTML =
        '<p class="px-2 pb-1 pt-1 text-[11px] font-semibold text-app-dim">' + L("변경 기록", "History") + '</p>' +
        '<div class="animate-rise rounded-md border border-app-line bg-white/[0.03] p-2"><p class="font-medium">Revert “' + L("웨이브 1 메시지와 저글링 스폰 추가", "Add wave 1 message and Zergling spawn") + '”</p><p class="mt-0.5 text-[10px] text-app-dim">' + L("방금", "just now") + '</p></div>' +
        '<div class="mt-1.5 rounded-md border border-app-line p-2 opacity-60"><p class="font-medium">' + L("웨이브 1 메시지와 저글링 스폰 추가", "Add wave 1 message and Zergling spawn") + '</p><p class="mt-0.5 text-[10px] text-app-dim">' + L("1분 전 · 세션 a3f · 요청 7", "1 min ago · session a3f · request 7") + '</p></div>';
      captionEl.textContent = L("파일이 요청 전 상태로 돌아왔습니다. 원래 커밋은 기록에 그대로 남습니다.", "The files are back to how they were before the request. The original commit stays in the history.");
    },
  };

  // =============================================================== 02 MAP
  SCENARIOS.map = {
    chapters: L(["영역 선택", "요청", "후보 만들기", "검증된 Apply", "Undo"], ["Select area", "Request", "Candidate", "Verified Apply", "Undo"]),
    intro: L("맵 에이전트 창에서 작업할 영역을 드래그해 선택 영역으로 저장합니다.", "In the Map Agent window, drag over the area to work on and save it as a selection."),
    build: function () {
      var ui = mapWindow({ map: "wave-defense.scx", badge: L("Map 세션", "Map session"), chat: true });
      ui.map = baseMap();
      ui.orig = cloneMap(ui.map);
      drawMap(ui.canvas, ui.map);
      return ui;
    },
    run: async function (ui) {
      var chat = ui.chat;
      var sel = { x: 21, y: 16, w: 1, h: 1, color: "#34d399", dash: true, fill: "rgba(52,211,153,0.10)" };
      await sleep(500);
      for (var i = 1; i <= 12; i++) {
        sel.w = Math.min(12, i);
        sel.h = Math.min(9, Math.ceil(i * 0.75));
        drawMap(ui.canvas, ui.map, { rects: [sel] });
        await sleep(60);
      }
      sel.label = L("중앙 공터", "Clearing");
      drawMap(ui.canvas, ui.map, { rects: [sel] });
      ui.palette.innerHTML =
        '<div class="animate-rise flex items-center gap-1.5 rounded-md bg-emerald-400/10 px-2 py-1.5 text-emerald-100"><span class="h-2 w-2 rounded-sm bg-emerald-400"></span>' + L("중앙 공터", "Clearing") + '<span class="ml-auto text-[10px] text-app-dim">12×9</span></div>';

      chapter(1, L("@로 선택 영역을 멘션하면, 에이전트는 그 영역 안만 바꿀 수 있습니다.", "Mention a selection with @ and the agent can change only what’s inside it."));
      await chat.ask(L("@중앙공터 에 언덕을 하나 만들고 가장자리에 나무를 몇 그루 심어줘", "Raise a hill in @Clearing and plant a few trees around the edge"));

      chapter(2, L("에이전트가 타일을 읽고, 초안에 ISOM 브러시로 언덕을 그린 뒤, 렌더링해 보며 다듬습니다. 원본은 아직 그대로입니다.", "The agent reads the tiles, paints the hill on a draft with the ISOM brush, then renders and refines it. The source map is untouched so far."));
      await chat.tool("map_selection_read", L("중앙 공터", "Clearing"), L("12×9 · 108타일 · 레이어 terrain, doodad", "12×9 · 108 tiles · layers terrain, doodad"));
      await chat.tool("map_terrain_read", "21,16 → 32,24", "108 tiles · dirt 96 · grass 12");
      await chat.tool("map_draft_begin", "", L("초안 r1", "draft r1"));
      var p = chat.tool("map_draft_patch", "terrain.isom_rect high-dirt", L("언덕 8×5 · 전이 타일 자동 생성", "8×5 hill · transitions generated"), 1600);
      ui.view("cand");
      var changes = hillChanges(23, 18, 8, 5);
      var cur = cloneMap(ui.map);
      await paint(ui.canvas, cur, changes, { rects: [sel] }, 5);
      await p;
      var trees = [[22, 17], [31, 17], [22, 23], [31, 23], [26, 24]];
      await chat.tool("map_draft_patch", "doodad.place ×5", L("나무 5그루", "5 trees"), 700);
      trees.forEach(function (t) {
        cur.doodads.push({ x: t[0], y: t[1], k: "tree" });
      });
      var diff = changes.map(function (c) {
        return c[0];
      });
      drawMap(ui.canvas, cur, { rects: [sel], diff: diff });
      await chat.tool("map_draft_render", "", L("렌더 확인 · 언덕 경사로 방향 적절", "render checked · ramp faces the right way"));
      await chat.tool("map_draft_analyze", "", L('통행 가능 · 변경 45타일 · <span class="text-app-fg">영역 밖 변경 0</span>', 'walkable · 45 tiles changed · <span class="text-app-fg">0 outside the area</span>'));
      await chat.tool("map_candidate_finalize", "", L("후보 r1 확정", "candidate r1 finalized"));
      ui.rev.textContent = L("r1 · 후보", "r1 · candidate");
      ui.rev.classList.add("border-emerald-400/40", "text-emerald-200");
      await chat.say(L("언덕과 나무 5그루를 후보로 만들었습니다. 확인 후 Apply해 주세요.", "The hill and 5 trees are ready as a candidate. Take a look, then Apply."));

      chapter(3, L("Apply를 누르면 백업, 검증, 원자적 교체가 차례로 진행됩니다. 하나라도 실패하면 백업으로 복원합니다.", "Apply runs a backup, verification and an atomic replace in turn. If any step fails, the backup is restored."));
      await gate(ui.apply, 3200);
      var confirm = dialog(
        ui.host,
        '<p class="font-semibold">' + L("검증된 후보 전체를 원본 SCX에 Apply할까요?", "Apply the whole verified candidate to the source SCX?") + '</p><p class="mt-1.5 text-xs text-app-dim">wave-defense.scx · terrain 45 · doodad 5</p>' +
          '<div class="mt-3 flex justify-end gap-2"><span class="rounded-md border border-app-line px-3 py-1 text-[12px] text-app-dim">' + L("취소", "Cancel") + '</span><button type="button" data-ok class="rounded-md bg-app-primary px-3 py-1 text-[12px] font-semibold text-app-primary-fg">Apply</button></div>'
      );
      await gate(confirm.querySelector("[data-ok]"), 1600);
      confirm.remove();
      ui.apply.disabled = true;
      var cl = checklist(ui.host, L(["빌드 중이 아님", "SCMDraft 공유 잠금 없음", "전체 백업 · 여유 공간", "CHK 재추출 · 변경 범위 검증", "원자적 교체"], ["No build running", "No SCMDraft share lock", "Full backup · free space", "CHK re-read · scope check", "Atomic replace"]));
      await cl.run(450);
      ui.map = cur;
      drawMap(ui.canvas, ui.map);
      ui.view("orig");
      ui.rev.textContent = L("r0 · 적용됨", "r0 · applied");
      await sleep(1600);
      cl.box.remove();

      chapter(4, L("Apply한 뒤에도 맵 창의 Undo가 백업해 둔 원본 바이트를 그대로 복원합니다.", "Even after Apply, Undo in the map window restores the backed-up original bytes exactly."));
      ui.undo.disabled = false;
      await gate(ui.undo, 3600);
      var confirmU = dialog(
        ui.host,
        '<p class="font-semibold">' + L("마지막 Apply의 full backup bytes를 원본에 복원할까요?", "Restore the last Apply’s full backup bytes to the source map?") + '</p><div class="mt-3 flex justify-end gap-2"><span class="rounded-md border border-app-line px-3 py-1 text-[12px] text-app-dim">' + L("취소", "Cancel") + '</span><button type="button" data-ok class="rounded-md bg-app-primary px-3 py-1 text-[12px] font-semibold text-app-primary-fg">' + L("복원", "Restore") + '</button></div>'
      );
      await gate(confirmU.querySelector("[data-ok]"), 1500);
      confirmU.remove();
      ui.undo.disabled = true;
      ui.map = cloneMap(ui.orig);
      drawMap(ui.canvas, ui.map, { rects: [sel] });
      ui.rev.textContent = L("r0 · 기준", "r0 · base");
      ui.rev.classList.remove("border-emerald-400/40", "text-emerald-200");
      chat.note(L("원본 맵을 Apply 이전 바이트로 복원했습니다.", "Restored the source map to its pre-Apply bytes."));
      captionEl.textContent = L("원본 맵이 Apply 이전과 바이트 단위로 같아졌습니다.", "The source map is byte-for-byte what it was before Apply.");
    },
  };

  // ============================================================== 03 TEAM
  SCENARIOS.team = {
    chapters: L(["요청", "코드 작업", "팀 Map 세션", "검사·Apply", "빌드"], ["Request", "Code", "Team Map session", "Inspect & Apply", "Build"]),
    intro: L("코드와 지형이 함께 필요한 요청은 EPS 세션이 코드를 맡고, 지형은 팀 Map 세션에 넘깁니다.", "When a request needs both code and terrain, the EPS session writes the code and hands the terrain to a team Map session."),
    build: function () {
      var root = h("div", "grid gap-3 lg:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]");
      var eps = windowFrame("eud-agent — wave-defense", L("EPS 세션", "EPS session"));
      eps.classList.add("flex", "flex-col");
      var chat = chatPanel(L("세션 · 보스방", "Session · Boss room"));
      chat.el.classList.add("h-[520px]", "lg:h-auto", "lg:flex-1");
      eps.appendChild(chat.el);
      var mw = mapWindow({ map: "wave-defense.scx", badge: L("팀 작업 대기", "Awaiting team task"), chat: false });
      mw.root.classList.add("transition-opacity", "duration-500", "opacity-40");
      var logBox = h("div", "mock-scroll h-28 space-y-1 border-t border-app-line bg-black/20 p-2.5 font-mono text-[11px] text-app-dim");
      logBox.innerHTML = '<p class="text-app-dim/60">' + L("팀 작업이 없습니다.", "No team tasks yet.") + '</p>';
      mw.root.appendChild(logBox);
      root.appendChild(eps);
      root.appendChild(mw.root);
      mw.map = baseMap();
      drawMap(mw.canvas, mw.map);
      return { root: root, chat: chat, mw: mw, log: logBox };
    },
    run: async function (ui) {
      var chat = ui.chat, mw = ui.mw;
      function mlog(html) {
        if (ui.log.firstChild && ui.log.firstChild.classList.contains("text-app-dim/60")) ui.log.innerHTML = "";
        ui.log.appendChild(h("p", "animate-rise", html));
        ui.log.scrollTop = ui.log.scrollHeight;
      }
      await sleep(500);
      await chat.ask(L("보스방을 만들고 싶어. 맵 왼쪽 위를 보스 아레나로 꾸며주고, 보스가 나오면 경고 메시지를 띄워줘", "I want a boss room. Turn the top-left of the map into a boss arena, and show a warning when the boss appears"));

      chapter(1, L("EPS 세션은 트리거 코드를 직접 작성합니다. 지형·유닛·두다드는 직접 배치하지 않습니다.", "The EPS session writes the trigger code itself. It never places terrain, units or doodads directly."));
      await chat.tool("fs_read", "src/main.eps", L("10줄", "10 lines"));
      await chat.tool("fs_write", "src/boss.eps", L("보스 등장 · 경고 메시지 · 로케이션 boss_spawn 사용", "boss entrance · warning message · uses location boss_spawn"));
      await chat.say(L("아레나 지형은 팀 Map 세션에 맡기겠습니다. 가운데는 보스 등장 자리라 비워 두도록 요청합니다.", "I’ll hand the arena terrain to a team Map session, asking it to keep the center clear for the boss entrance."));

      chapter(2, L("map_task_request는 목표 문장과 작업 영역·보호 영역 사각형을 넘깁니다. 검증기는 영역 밖 변경을 거부합니다.", "map_task_request passes a goal plus target and protect rectangles. The verifier rejects any change outside them."));
      var req = chat.push(
        h(
          "div",
          "animate-rise rounded-md border border-sky-400/30 bg-sky-400/5 px-2.5 py-2 text-[12px]",
          '<div class="flex items-center gap-2"><span data-st>' + SPINNER + '</span><span class="font-mono text-sky-200">map_task_request</span><span data-status class="ml-auto rounded bg-white/5 px-1.5 text-[10px] text-app-dim">queued</span></div>' +
            '<p class="mt-1.5 leading-relaxed text-app-fg/85">' + L("“맵 왼쪽 위를 보스 아레나처럼 꾸며줘. 가운데는 보스 등장 자리라 비워 둬.”", "“Make the top-left of the map look like a boss arena. Keep the center clear for the boss entrance.”") + '</p>' +
            '<p class="mt-1 font-mono text-[10px] text-app-dim">target 1,1 → 13,11 · protect 5,4 → 9,8 · layers terrain, doodad</p>'
        )
      );
      var status = req.querySelector("[data-status]");
      await sleep(700);
      mw.root.classList.remove("opacity-40");
      mw.root.querySelector(".truncate").textContent = L("맵 에이전트 — 팀 작업 · EPS 세션", "Map Agent — team task · EPS session");
      var target = { x: 1, y: 1, w: 12, h: 10, color: "#34d399", dash: true, label: "target" };
      var protect = { x: 5, y: 4, w: 4, h: 4, color: "#fbbf24", dash: true, fill: "rgba(251,191,36,0.12)", label: "protect" };
      var ov = { rects: [target, protect] };
      drawMap(mw.canvas, mw.map, ov);
      status.textContent = "running";
      mlog('<span class="text-sky-300">map_terrain_read</span> 1,1 → 13,11');
      await sleep(700);
      mlog('<span class="text-sky-300">map_draft_patch</span> terrain.isom_rect stone');
      var cur = cloneMap(mw.map);
      var ch = [];
      for (var y = 1; y < 11; y++)
        for (var x = 1; x < 13; x++) {
          var inProtect = x >= 5 && x < 9 && y >= 4 && y < 8;
          if (inProtect) continue;
          var edge = x === 1 || y === 1 || x === 12 || y === 10;
          ch.push([y * W + x, edge ? "cliff" : "stone"]);
        }
      await paint(mw.canvas, cur, ch, ov, 10);
      mlog('<span class="text-sky-300">map_draft_patch</span> doodad.place torch ×4');
      [[3, 3], [10, 3], [3, 8], [10, 8]].forEach(function (t) {
        cur.doodads.push({ x: t[0], y: t[1], k: "torch" });
      });
      drawMap(mw.canvas, cur, ov);
      await sleep(600);
      mlog('<span class="text-sky-300">map_draft_render</span> · <span class="text-sky-300">map_draft_analyze</span> ' + L("보호 영역 변경 0", "0 changes in protect"));
      await sleep(700);
      mlog('<span class="text-emerald-300">candidate_ready</span> r1');
      mw.rev.textContent = L("r1 · 후보", "r1 · candidate");
      mw.view("cand");
      status.textContent = "candidate_ready";
      req.querySelector("[data-st]").innerHTML = icon("check", "h-3.5 w-3.5 text-app-primary");
      var diff = ch.map(function (c) {
        return c[0];
      });
      drawMap(mw.canvas, cur, { rects: [target, protect], diff: diff });

      chapter(3, L("EPS 세션이 후보를 직접 검사한 뒤 적용합니다. Map 창의 Apply를 먼저 눌러도 같은 경로로 적용됩니다.", "The EPS session inspects the candidate, then applies it. Press Apply in the Map window first and it goes the same path."));
      await chat.tool("map_task_diff", "", L("terrain 116 · doodad 4 · protect 변경 0", "terrain 116 · doodad 4 · protect 0 changed"));
      await chat.tool("map_task_render", "", L("아레나 렌더 확인", "arena render checked"));
      var byUser = await gate(mw.apply, 2400);
      mw.apply.disabled = true;
      if (byUser) {
        chat.note(L("사용자가 Map 창에서 후보를 Apply했습니다.", "You applied the candidate from the Map window."));
        mlog('<span class="text-emerald-300">applied</span> · ' + L("Map 창 Apply", "Map window Apply"));
      } else {
        await chat.tool("map_task_apply", "r1", L("백업 · 검증 · 원자적 교체 완료", "backed up · verified · atomically replaced"), 1300);
        mlog('<span class="text-emerald-300">applied</span> · map_task_apply');
      }
      mw.map = cur;
      drawMap(mw.canvas, cur, { rects: [protect] });
      mw.rev.textContent = L("r0 · 적용됨", "r0 · applied");
      mw.view("orig");
      mw.undo.disabled = false;

      chapter(4, L("지형과 코드가 모두 들어간 뒤 빌드합니다. Undo는 Map 창에 계속 남아 있습니다.", "Once terrain and code are both in, it builds. Undo stays available in the Map window."));
      await chat.tool("build_run", "", L('<span class="text-app-primary">성공</span> · 오류 0 · 경고 0', '<span class="text-app-primary">Success</span> · 0 errors · 0 warnings'), 2200);
      await chat.say(L("보스 아레나를 적용하고 boss.eps를 추가했습니다. 보스가 boss_spawn에 나오면 모든 플레이어에게 경고가 뜹니다.", "Applied the boss arena and added boss.eps. When the boss appears at boss_spawn, every player gets a warning."));
    },
  };

  // =============================================================== 04 DAT
  var UNITS = [
    { id: 0, n: "Terran Marine", k: L("마린", "Marine"), c: "#3b82f6", hp: 40, ar: 0, m: 50, g: 0, t: 360, s: 7 },
    { id: 32, n: "Terran Firebat", k: L("파이어뱃", "Firebat"), c: "#3b82f6", hp: 50, ar: 1, m: 50, g: 25, t: 360, s: 7 },
    { id: 34, n: "Terran Medic", k: L("메딕", "Medic"), c: "#3b82f6", hp: 60, ar: 1, m: 50, g: 25, t: 450, s: 9 },
    { id: 37, n: "Zerg Zergling", k: L("저글링", "Zergling"), c: "#a855f7", hp: 35, ar: 0, m: 50, g: 0, t: 420, s: 5 },
    { id: 38, n: "Zerg Hydralisk", k: L("히드라리스크", "Hydralisk"), c: "#a855f7", hp: 80, ar: 0, m: 75, g: 25, t: 420, s: 6 },
    { id: 65, n: "Protoss Zealot", k: L("질럿", "Zealot"), c: "#eab308", hp: 100, ar: 1, m: 100, g: 0, t: 600, s: 7 },
    { id: 66, n: "Protoss Dragoon", k: L("드라군", "Dragoon"), c: "#eab308", hp: 100, ar: 1, m: 125, g: 50, t: 750, s: 8 },
  ];
  SCENARIOS.dat = {
    chapters: L(["DAT 위키", "요청", "dat_patch", "저장되는 값", "빌드"], ["DAT wiki", "Request", "dat_patch", "What’s stored", "Build"]),
    intro: L("DAT 위키에서 유닛·무기·업그레이드의 원본 값과 프로젝트 값을 나란히 봅니다. 목록은 직접 눌러 볼 수 있습니다.", "The DAT wiki shows stock and project values side by side for units, weapons and upgrades. Click through the list yourself."),
    build: function () {
      var ui = mainWindow({ project: "wave-defense", badge: L("EPS 세션", "EPS session"), side: "dat", chatTitle: L("세션 · 밸런스", "Session · Balance") });
      ui.overrides = {};
      ui.selected = 0;
      ui.side.innerHTML =
        '<div class="mb-1.5 flex items-center gap-1.5 rounded-md border border-app-line bg-app-bg px-2 py-1 text-[11px] text-app-dim">' + icon("search", "h-3 w-3") + "units</div>" +
        '<div data-list class="flex flex-col gap-0.5"></div>';
      ui.list = ui.side.querySelector("[data-list]");
      ui.center.innerHTML =
        '<div class="flex h-9 items-end border-b border-app-line bg-black/20 px-2"><span class="flex items-center gap-1.5 rounded-t-md bg-app-bg px-3 py-1.5 text-[12px]">' + icon("table", "h-3 w-3") + L("DAT 위키", "DAT wiki") + '</span><span data-json-tab class="hidden items-center gap-1.5 px-3 py-1.5 text-[12px] text-app-dim">' + icon("file", "h-3 w-3") + "standard.json</span></div>" +
        '<div data-detail class="mock-scroll min-h-0 flex-1 p-3"></div>';
      ui.detail = ui.center.querySelector("[data-detail]");
      ui.jsonTab = ui.center.querySelector("[data-json-tab]");
      ui.renderList = function () {
        ui.list.innerHTML = "";
        UNITS.forEach(function (u, i) {
          var b = h(
            "button",
            "flex w-full items-center gap-2 rounded-md px-1.5 py-1 text-left transition-colors " + (i === ui.selected ? "bg-white/10 text-app-fg" : "text-app-dim hover:bg-white/5"),
            '<span class="grid h-6 w-6 shrink-0 place-items-center rounded text-[9px] font-bold text-white" style="background:' + u.c + '99">' + u.id + '</span><span class="truncate">' + esc(u.k) + "</span>" +
              (ui.overrides[u.id] ? '<span class="ml-auto h-1.5 w-1.5 shrink-0 rounded-full bg-amber-300" title="' + L("재정의됨", "Overridden") + '"></span>' : "")
          );
          b.type = "button";
          b.setAttribute("aria-pressed", i === ui.selected ? "true" : "false");
          b.addEventListener("click", function () {
            ui.selected = i;
            ui.renderList();
            ui.renderDetail();
          });
          ui.list.appendChild(b);
        });
      };
      ui.renderDetail = function (flash) {
        var u = UNITS[ui.selected];
        var ov = ui.overrides[u.id] || {};
        var rows = [
          ["Hit Points", L("체력", ""), u.hp],
          ["Armor", L("방어력", ""), u.ar],
          ["Mineral Cost", L("광물", ""), u.m],
          ["Vespene Cost", L("가스", ""), u.g],
          ["Build Time", L("생산 시간(프레임)", ""), u.t],
          ["Sight Range", L("시야", ""), u.s],
        ];
        ui.detail.innerHTML =
          '<div class="flex items-center gap-3"><span class="grid h-11 w-11 place-items-center rounded-lg text-xs font-bold text-white" style="background:' + u.c + '">' + u.id + '</span><div><p class="font-semibold">' + esc(u.k) + '</p><p class="font-mono text-[11px] text-app-dim">units · ' + esc(u.n) + " · id " + u.id + "</p></div></div>" +
          '<table class="mt-3 w-full text-[12px]"><thead><tr class="text-left text-[11px] text-app-dim"><th class="py-1 font-medium">' + L("필드", "Field") + '</th><th class="py-1 text-right font-medium">' + L("원본", "Stock") + '</th><th class="py-1 text-right font-medium">' + L("프로젝트", "Project") + '</th></tr></thead><tbody>' +
          rows
            .map(function (r) {
              var o = ov[r[0]];
              var hit = o != null;
              return '<tr class="border-t border-app-line ' + (hit && flash ? "animate-rise bg-amber-400/10" : hit ? "bg-amber-400/5" : "") + '"><td class="py-1.5"><span>' + r[1] + '</span> <span class="font-mono text-[10px] text-app-dim">' + r[0] + '</span></td><td class="py-1.5 text-right font-mono text-app-dim">' + r[2] + '</td><td class="py-1.5 text-right font-mono ' + (hit ? "font-semibold text-amber-200" : "text-app-dim/60") + '">' + (hit ? o : "—") + "</td></tr>";
            })
            .join("") +
          "</tbody></table>" +
          '<p class="mt-3 text-[11px] leading-relaxed text-app-dim">' + L("위키는 읽기 전용입니다. 값은 dat_patch로만 바뀌고, 변경 기록에 남습니다.", "The wiki is read-only. Values change only through dat_patch, and every change lands in History.") + '</p>';
      };
      ui.renderList();
      ui.renderDetail();
      return ui;
    },
    run: async function (ui) {
      var chat = ui.chat;
      await sleep(2200);
      chapter(1, L("원하는 밸런스를 말로 요청합니다.", "Ask for the balance you want, in plain words."));
      await chat.ask(L("마린 체력을 60으로 올리고 생산 시간을 20초로 줄여줘", "Raise Marine HP to 60 and cut its build time to 20 seconds"));

      chapter(2, L("에이전트는 현재 값을 확인한 뒤 before/after 쌍으로 패치합니다. before가 원본과 다르면 거부됩니다.", "The agent checks the current values, then patches with before/after pairs. A before that doesn’t match stock is rejected."));
      await chat.tool("dat_get", "units 0 · Hit Points, Build Time", "Hit Points 10240 (40 × 256) · Build Time 360");
      await chat.say(L("체력은 1/256 단위라 60 × 256 = 15360으로, 생산 시간은 20초 × 15프레임 = 300으로 바꿉니다.", "HP is stored in 1/256 units, so 60 × 256 = 15360; build time becomes 20 s × 15 frames = 300."));
      await chat.tool(
        "dat_patch",
        "2 changes",
        '<span class="font-mono">units[0].Hit Points 10240 → 15360</span><br><span class="font-mono">units[0].Build Time 360 → 300</span>',
        1000
      );
      ui.overrides[0] = { "Hit Points": 60, "Build Time": 300 };
      ui.selected = 0;
      ui.renderList();
      ui.renderDetail(true);
      await sleep(2000);

      chapter(3, L("프로젝트에는 바뀐 값만 저장됩니다. 나머지는 버전이 맞는 원본 카탈로그 값을 그대로 씁니다.", "The project stores only what changed. Everything else comes from the version-matched stock catalog."));
      ui.jsonTab.classList.remove("hidden");
      ui.jsonTab.classList.add("flex");
      var q = function (s) { return '<span class="tok-s">"' + s + '"</span>'; };
      ui.detail.innerHTML =
        '<p class="mb-2 font-mono text-[11px] text-app-dim">dat/standard.json</p><div class="font-mono text-[12px] leading-[1.7]">' +
        [
          "{",
          "  " + q("units") + ": {",
          "    " + q("0") + ": {",
          "      " + q("Hit Points") + ": { " + q("before") + ": " + N("10240") + ", " + q("after") + ": " + N("15360") + " },",
          "      " + q("Build Time") + ": { " + q("before") + ": " + N("360") + ", " + q("after") + ": " + N("300") + " }",
          "    }",
          "  }",
          "}",
        ]
          .map(function (l, i) {
            return '<div class="code-line ' + (i >= 3 && i <= 4 ? "add" : "") + '"><span class="ln">' + (i + 1) + '</span><span class="mark">' + (i >= 3 && i <= 4 ? "+" : "") + "</span><span>" + l + "</span></div>";
          })
          .join("") +
        "</div>";
      await sleep(3200);
      ui.jsonTab.classList.add("hidden");
      ui.jsonTab.classList.remove("flex");
      ui.renderDetail();

      chapter(4, L("빌드할 때 이 오버라이드로 DAT 플러그인이 만들어집니다. 바뀐 게 없는 DAT는 플러그인도 만들지 않습니다.", "At build time these overrides become a DAT plugin. A DAT with no changes gets no plugin at all."));
      await chat.tool("build_run", "", L('<span class="text-app-primary">성공</span> · units.dat 패치 1개 생성', '<span class="text-app-primary">Success</span> · 1 units.dat patch generated'), 2000);
      await chat.say(L("마린 체력 60, 생산 시간 20초로 바꾸고 빌드까지 마쳤습니다.", "Marine HP is now 60 and build time 20 seconds, and the build passed."));
    },
  };

  // Start whichever demo the hash names, else the first.
  var initial = (location.hash.match(/^#demo-(\w+)/) || [])[1];
  start(SCENARIOS[initial] ? initial : "eps");
  setPaused(userPaused);

  // ======================================================= architecture svg
  var svg = document.getElementById("arch");
  if (svg) {
    var NODES = {
      panel: [30, 30, L("React 패널", "React panel"), L("메인 창 · Map 창", "Main · Map windows"), L("채팅, 에디터, DAT 위키, 변경 기록, 맵 캔버스가 있는 화면입니다. 모든 동작은 타입이 정해진 Tauri 명령으로 Rust에 요청합니다.", "The screens: chat, editor, DAT wiki, history and the map canvas. Every action goes to Rust as a typed Tauri command.")],
      ipc: [250, 30, "Tauri IPC", "typed invoke / listen", L("패널과 Rust 백엔드 사이의 경계입니다. 대화 이벤트는 세션별로 전달되고, 프로젝트가 없으면 작성 기능만 막습니다.", "The boundary between the panel and the Rust backend. Conversation events are routed per session; without a project, only authoring is blocked.")],
      engine: [470, 30, L("에이전트 엔진", "Agent engine"), L("세션 · 턴 · 자율 실행", "Sessions · turns · runs"), L("세션마다 대화, 요청, 자율 실행 상태를 가집니다. 턴이 끝나면 git 커밋으로 정산하고, 중단된 요청은 사용자가 이어가거나 다시 시작하게 둡니다.", "Each session holds its conversation, requests and autonomous-run state. A finished turn settles as a git commit; an interrupted request waits for you to continue or restart it.")],
      providers: [680, 30, L("AI 제공자 ×6", "AI providers ×6"), "Codex · Claude · …", L("Codex, Claude Code, Antigravity, OpenCode Go, OpenCode Zen, Ollama. 각 CLI·API의 공식 세션을 그대로 쓰고, 공통 런타임이 취소, 출력 한도, 도구 게이트를 관리합니다.", "Codex, Claude Code, Antigravity, OpenCode Go, OpenCode Zen and Ollama, each through its own official CLI or API session. A shared runtime handles cancellation, output limits and the tool gate.")],
      mapagent: [250, 150, "Map Agent", L("초안 · 후보 · MapSafe", "Drafts · MapSafe Apply"), L("ISOM 브러시, 두다드, 유닛, 사운드를 초안에 그린 뒤 검증된 후보로 확정합니다. Apply는 백업, 검증, 원자적 교체를 거칩니다.", "Paints ISOM terrain, doodads, units and sounds on a draft, then finalizes a verified candidate. Apply goes through backup, verification and an atomic replace.")],
      tools: [470, 150, L("도구 런타임", "Tool runtime"), L("스키마 검증 · 쓰기 직렬화", "Schemas · serial writes"), L("모든 도구 호출은 스키마를 검증한 뒤 실행됩니다. 여러 세션의 프로젝트 쓰기는 한 번에 하나씩 처리됩니다.", "Every tool call is checked against its schema before it runs. Project writes from many sessions are applied one at a time.")],
      rag: [680, 150, L("참고 문서 RAG", "Docs RAG"), L("bge-m3 · 프로세스 내", "bge-m3 · in-process"), L("EUD 커뮤니티 문서를 앱 프로세스 안에서 어휘+의미 검색합니다. 사이드바의 참고 문서 탭과 에이전트의 search_docs가 같은 인덱스를 씁니다.", "Lexical plus semantic search over EUD community docs, inside the app process. The sidebar’s Docs tab and the agent’s search_docs share one index.")],
      scx: [30, 270, L("원본 맵", "Source map"), "maps/*.scx", L("지형과 유닛이 들어 있는 원본 맵입니다. 맵 세션은 이 파일을 따라가며, 외부에서 저장되면 후보를 새 원본 위에 다시 쌓습니다.", "The source map with terrain and units. Map sessions follow this file; when it’s saved elsewhere, candidates are rebuilt on the new source.")],
      git: [250, 270, L("git 기록", "git history"), L("턴마다 커밋", "One commit per turn"), L("앱이 프로젝트 폴더를 git으로 관리합니다. 요청 하나가 커밋 하나가 되고, 되돌리기는 반대 커밋을 기록합니다.", "The app keeps the project folder in git. One request is one commit, and a revert records the reverse commit.")],
      project: [470, 270, L("Native 프로젝트", "Native project"), "project.eap · src · dat", L("작성의 기준이 되는 파일들입니다. 매니페스트, epScript·Python 소스, 바뀐 DAT 값만 담는 JSON 다섯 개로 이루어집니다.", "The files authoring is based on: the manifest, epScript and Python sources, and five JSON files holding only the DAT values that changed.")],
      generator: [680, 270, L("빌드 생성기", "Build generator"), L("EDS · DAT 플러그인", "EDS · DAT plugins"), L("프로젝트에서 EDS와 DAT/TBL 플러그인을 항상 같은 결과로 생성합니다. 바뀐 DAT가 없으면 플러그인도 없습니다.", "Generates the EDS and DAT/TBL plugins from the project, deterministically. No changed DAT means no plugin.")],
      game: [250, 390, L("스타크래프트", "StarCraft"), "Maps/eud-agent/", L("빌드된 맵을 설치된 스타크래프트의 Maps 폴더로 복사해 바로 테스트할 수 있게 합니다. 복사 실패는 경고일 뿐 빌드 실패가 아닙니다.", "Copies the built map into your StarCraft Maps folder so you can test it right away. A failed copy is a warning, not a failed build.")],
      output: [470, 390, L("출력 맵", "Output map"), "build/[EUD]*.scx", L("euddraft가 새로 만든 출력 맵입니다. 이 파일이 새로 생겨야 빌드 성공입니다.", "The output map euddraft just wrote. A build passes only if this file is freshly created.")],
      euddraft: [680, 390, "euddraft", L("제한된 하위 프로세스", "Bounded subprocess"), L("제한된 하위 프로세스로 실행합니다. 전체 출력은 build.log에 남기고, 모델에게는 파일·줄 단위 진단과 짧은 발췌만 줍니다.", "Runs as a bounded subprocess. The full output goes to build.log; the model gets file/line diagnostics and a short excerpt.")],
    };
    var NW = 150, NH = 58;
    var EDGES = [
      ["panel", "ipc"], ["ipc", "engine"], ["engine", "providers", 1], ["engine", "tools"], ["tools", "rag"],
      ["tools", "mapagent"], ["tools", "project"], ["mapagent", "scx"], ["project", "git"], ["project", "generator"],
      ["generator", "euddraft"], ["euddraft", "output"], ["output", "game"],
    ];
    var FLOWS = {
      build: {
        nodes: ["panel", "ipc", "engine", "providers", "tools", "project", "git", "generator", "euddraft", "output", "game"],
        steps: [L("패널에서 요청을 보냅니다", "You send a request from the panel"), L("제공자가 fs_read·fs_edit 도구를 호출합니다", "The provider calls fs_read and fs_edit"), L("도구 런타임이 프로젝트 파일을 씁니다", "The tool runtime writes the project files"), L("build_run이 EDS를 생성해 euddraft를 실행합니다", "build_run generates the EDS and runs euddraft"), L("출력 맵이 생기면 Maps 폴더로 복사합니다", "The output map is copied to the Maps folder"), L("턴이 끝나면 git 커밋으로 남습니다", "The finished turn is saved as a git commit")],
      },
      map: {
        nodes: ["panel", "ipc", "engine", "providers", "tools", "mapagent", "scx"],
        steps: [L("Map 창에서 선택 영역과 함께 요청합니다", "You ask in the Map window, with a selection"), L("초안에 그리고 렌더링하며 다듬습니다", "The agent paints a draft, renders and refines it"), L("영역 밖 변경이 없는지 검증해 후보로 확정합니다", "It verifies nothing changed outside the area and finalizes a candidate"), L("Apply: 백업 → 검증 → 원본 맵 원자적 교체", "Apply: backup → verify → atomic replace of the source map"), L("Undo: 백업 바이트로 복원합니다", "Undo: restore the backed-up bytes")],
      },
      team: {
        nodes: ["engine", "tools", "mapagent", "scx", "project", "generator", "euddraft", "output"],
        steps: [L("EPS 세션이 코드를 작성합니다", "The EPS session writes the code"), L("map_task_request로 팀 Map 세션에 목표와 영역을 넘깁니다", "map_task_request hands a goal and areas to a team Map session"), L("팀 세션이 새 대화로 후보를 만듭니다", "The team session builds a candidate in a fresh conversation"), L("EPS 세션이 diff·렌더를 검사하고 map_task_apply", "The EPS session checks the diff and render, then calls map_task_apply"), L("코드와 맵을 함께 빌드합니다", "Code and map are built together")],
      },
      rag: {
        nodes: ["engine", "providers", "tools", "rag"],
        steps: [L("제공자가 search_docs를 호출합니다", "The provider calls search_docs"), L("어휘 검색 후 bge-m3 의미 검색으로 순위를 정합니다", "Lexical search, then bge-m3 semantic ranking"), L("근거 문서 조각을 링크와 함께 돌려줍니다", "Matching doc passages come back with links")],
      },
    };

    var edgesG = document.getElementById("arch-edges");
    var nodesG = document.getElementById("arch-nodes");
    var NS = "http://www.w3.org/2000/svg";
    function sv(tag, attrs, parent) {
      var n = document.createElementNS(NS, tag);
      for (var k in attrs) n.setAttribute(k, attrs[k]);
      if (parent) parent.appendChild(n);
      return n;
    }
    function edgePath(a, b) {
      var A = NODES[a], B = NODES[b];
      var ax = A[0] + NW / 2, ay = A[1] + NH / 2, bx = B[0] + NW / 2, by = B[1] + NH / 2;
      if (A[1] === B[1]) {
        var dir = bx > ax ? 1 : -1;
        return "M" + (ax + (dir * NW) / 2) + " " + ay + " H" + (bx - (dir * NW) / 2 - dir * 2);
      }
      if (A[0] === B[0]) {
        var dv = by > ay ? 1 : -1;
        return "M" + ax + " " + (ay + (dv * NH) / 2) + " V" + (by - (dv * NH) / 2 - dv * 2);
      }
      var dx = bx > ax ? 1 : -1;
      return "M" + (ax + (dx * NW) / 2) + " " + ay + " H" + bx + " V" + (B[1] - 2);
    }
    var edgeEls = EDGES.map(function (e) {
      var p = sv("path", { d: edgePath(e[0], e[1]), class: "flow-edge" }, edgesG);
      if (e[2]) p.setAttribute("marker-start", "url(#arrow)");
      p.dataset.a = e[0];
      p.dataset.b = e[1];
      return p;
    });
    var nodeEls = {};
    Object.keys(NODES).forEach(function (id) {
      var n = NODES[id];
      var g = sv("g", { tabindex: "0", role: "button", "aria-label": n[2] + " — " + n[3], class: "cursor-pointer outline-none" }, nodesG);
      var rect = sv("rect", { x: n[0], y: n[1], width: NW, height: NH, rx: 10, fill: "#111a30", stroke: "#334155", "stroke-width": 1.5 }, g);
      var t1 = sv("text", { x: n[0] + NW / 2, y: n[1] + 25, "text-anchor": "middle", fill: "#e2e8f0", "font-size": 14, "font-weight": 600 }, g);
      t1.textContent = n[2];
      var t2 = sv("text", { x: n[0] + NW / 2, y: n[1] + 43, "text-anchor": "middle", fill: "#94a3b8", "font-size": 11 }, g);
      t2.textContent = n[3];
      nodeEls[id] = rect;
      function pick() {
        showNode(id);
      }
      g.addEventListener("click", pick);
      g.addEventListener("keydown", function (e) {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          pick();
        }
      });
      g.addEventListener("focus", function () {
        rect.setAttribute("stroke", "#7dd3fc");
      });
      g.addEventListener("blur", function () {
        applyHighlight();
      });
    });

    var detail = document.getElementById("arch-detail");
    var dTitle = detail.querySelector("[data-detail-title]");
    var dBody = detail.querySelector("[data-detail-body]");
    var dSteps = detail.querySelector("[data-flow-steps]");
    var flowBtns = Array.prototype.slice.call(document.querySelectorAll("[data-flow]"));
    var activeFlow = null, activeNode = null;

    function applyHighlight() {
      var set = activeFlow ? FLOWS[activeFlow].nodes : [];
      Object.keys(nodeEls).forEach(function (id) {
        var on = set.indexOf(id) >= 0;
        var sel = id === activeNode;
        nodeEls[id].setAttribute("stroke", sel ? "#7dd3fc" : on ? "#34d399" : "#334155");
        nodeEls[id].setAttribute("fill", sel ? "#10233a" : on ? "#0f2a24" : "#111a30");
      });
      edgeEls.forEach(function (p) {
        var on = set.indexOf(p.dataset.a) >= 0 && set.indexOf(p.dataset.b) >= 0;
        p.classList.toggle("active", on);
      });
    }
    function showNode(id) {
      activeNode = id;
      dTitle.textContent = NODES[id][2];
      dBody.textContent = NODES[id][4];
      applyHighlight();
    }
    function showFlow(name) {
      activeFlow = name;
      activeNode = null;
      flowBtns.forEach(function (b) {
        b.setAttribute("aria-pressed", b.dataset.flow === name ? "true" : "false");
      });
      var btn = flowBtns.filter(function (b) {
        return b.dataset.flow === name;
      })[0];
      dTitle.textContent = btn ? btn.textContent : "";
      dBody.textContent = L("초록색으로 켜진 경로를 따라 데이터가 이동합니다. 상자를 누르면 해당 구성 요소를 설명합니다.", "Data moves along the path lit in green. Click a box to see what that component does.");
      dSteps.innerHTML = FLOWS[name].steps
        .map(function (s, i) {
          return '<li class="flex gap-2.5"><span class="grid h-5 w-5 shrink-0 place-items-center rounded-full bg-emerald-400/15 text-[11px] font-semibold text-emerald-300">' + (i + 1) + '</span><span class="text-slate-300">' + esc(s) + "</span></li>";
        })
        .join("");
      applyHighlight();
    }
    flowBtns.forEach(function (b) {
      b.addEventListener("click", function () {
        showFlow(b.dataset.flow);
      });
    });
    showFlow("build");
  }

  // ============================================================ os tabs
  var osTabs = Array.prototype.slice.call(document.querySelectorAll("[data-os-tab]"));
  function showOs(os) {
    osTabs.forEach(function (t) {
      t.setAttribute("aria-selected", t.dataset.osTab === os ? "true" : "false");
    });
    document.querySelectorAll("[data-os-panel]").forEach(function (p) {
      p.hidden = p.dataset.osPanel !== os;
    });
  }
  osTabs.forEach(function (t) {
    t.addEventListener("click", function () {
      showOs(t.dataset.osTab);
    });
  });
  showOs(document.documentElement.getAttribute("data-os") === "mac" ? "mac" : "win");
})();
