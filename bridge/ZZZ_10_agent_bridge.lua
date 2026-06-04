-- ============================================================================
-- ZZZ_10_agent_bridge.lua : 외부 에이전트 <-> EUD Editor 3 파일 IPC 브리지 (v6)
--   v5: u8() 한글 UI 복원 + PANEL을 4기능 제어판으로 확장
--       (트리거 에디터 열기 / 새 eps 생성 / 목록 선택 열기 / 코드 적용)
--   v6: 프로젝트가 열리면(pjData nil->존재) 제어판 자동 표시. 닫히면 재무장.
--
-- 설치: <에디터>\Data\Lua\TriggerEditor\ 복사 후 재시작.
-- 통신: Data\agent\inbox\<n>.cmd  ->  outbox\<n>.result  ,  status.txt
-- 명령: PING STATUS DUMP / GET <경로> / SET <경로>\n<본문>
--       GETDAT <datname>|<param>|<objId> / SETDAT <datname>|<param>|<objId>|<value>
--       PANEL / BUILD / LUA
-- 규칙: UI 스레드(Tick) 전용 / 인덱스 프로퍼티 get_*() / enum 객체 / Array는 [i]
--       / 한글 표시는 u8() / 빌드중 skip
-- ============================================================================

local ok, initErr = pcall(function()
    luanet.load_assembly("mscorlib")
    luanet.load_assembly("System, Version=4.0.0.0, Culture=neutral, PublicKeyToken=b77a5c561934e089")
    luanet.load_assembly("WindowsBase, Version=4.0.0.0, Culture=neutral, PublicKeyToken=31bf3856ad364e35")
    luanet.load_assembly("PresentationCore, Version=4.0.0.0, Culture=neutral, PublicKeyToken=31bf3856ad364e35")
    luanet.load_assembly("PresentationFramework, Version=4.0.0.0, Culture=neutral, PublicKeyToken=31bf3856ad364e35")
    luanet.load_assembly("EUD Editor 3")
    -- WebView2 SDK DLLs are deployed NEXT TO THE EDITOR EXE (install_dropin);
    -- app-base probing makes load_assembly by simple name resolve them.
    luanet.load_assembly("Microsoft.Web.WebView2.Core")
    luanet.load_assembly("Microsoft.Web.WebView2.Wpf")

    local AppDomain  = luanet.import_type("System.AppDomain")
    local File       = luanet.import_type("System.IO.File")
    local Directory  = luanet.import_type("System.IO.Directory")
    local Path       = luanet.import_type("System.IO.Path")
    local DateTime   = luanet.import_type("System.DateTime")
    local Encoding   = luanet.import_type("System.Text.Encoding")
    local GlobalObj  = luanet.import_type("EUD_Editor_3.GlobalObj")
    local TEFile     = luanet.import_type("EUD_Editor_3.TEFile")
    local EFileType  = luanet.import_type("EUD_Editor_3.TEFile+EFileType")
    local WindowControl = luanet.import_type("EUD_Editor_3.WindowControl")
    local WMenus     = luanet.import_type("EUD_Editor_3.WindowMenu.WindowMenus")
    local DispatcherTimer = luanet.import_type("System.Windows.Threading.DispatcherTimer")
    local DispatcherPriority = luanet.import_type("System.Windows.Threading.DispatcherPriority")
    local TimeSpan   = luanet.import_type("System.TimeSpan")
    local Process    = luanet.import_type("System.Diagnostics.Process")
    local ProcessStartInfo = luanet.import_type("System.Diagnostics.ProcessStartInfo")
    -- WebView2 panel hosting (v6 WPF control panel replaced). Window reuses the
    -- v6 showPanel import idiom; WebView2 + CreationProperties from the SDK.
    local Window     = luanet.import_type("System.Windows.Window")
    local WebView2   = luanet.import_type("Microsoft.Web.WebView2.Wpf.WebView2")
    local CoreWebView2CreationProperties =
        luanet.import_type("Microsoft.Web.WebView2.Wpf.CoreWebView2CreationProperties")

    local baseDir   = tostring(AppDomain.CurrentDomain.BaseDirectory)
    local agentDir  = baseDir .. "Data\\agent\\"
    local inboxDir  = agentDir .. "inbox\\"
    local outboxDir = agentDir .. "outbox\\"
    Directory.CreateDirectory(inboxDir)
    Directory.CreateDirectory(outboxDir)

    -- u8: .lua 소스의 한글(Latin1 mojibake)을 올바른 유니코드로 복원
    local latin1 = Encoding.GetEncoding("iso-8859-1")
    local utf8   = Encoding.UTF8
    local function u8(s) return utf8:GetString(latin1:GetBytes(s)) end

    local function safestr(v) if v == nil then return "" end return tostring(v) end
    local function split(s, sep)
        local parts = {}
        for part in string.gmatch(s .. sep, "([^" .. sep .. "]*)" .. sep) do parts[#parts + 1] = part end
        return parts
    end
    local function getText(f) return safestr(f.Scripter:GetStringText()) end

    local function walk(node, prefix, visit)
        for i = 0, node.FolderCount - 1 do
            local sub = node:get_Folders(i)
            walk(sub, prefix .. safestr(sub.FileName) .. "/", visit)
        end
        for i = 0, node.FileCount - 1 do
            local f = node:get_Files(i)
            visit(prefix .. safestr(f.FileName), f)
        end
    end
    local function findFile(path)
        local pj = GlobalObj.pjData
        if pj == nil then return nil end
        local found = nil
        walk(pj.TEData.PFIles, "", function(p, f) if p == path then found = f end end)
        return found
    end

    -- ------------------------------------------------------------------
    -- Server lifecycle: agent.cfg -> spawn python server -> ready/respawn.
    -- KopiLua has no JSON lib; the cfg/ready files are flat JSON parsed with
    -- string.match. agent.cfg paths are JSON-escaped (\\), so unescape "\\".
    -- ------------------------------------------------------------------
    local cfgPath   = agentDir .. "agent.cfg"
    local readyPath = agentDir .. "server.ready"
    local errLogPath = agentDir .. "bridge_error.log"
    local function nowIso() return tostring(DateTime.Now:ToString("o")) end
    local function logError(msg)
        pcall(function()
            Directory.CreateDirectory(agentDir)
            File.AppendAllText(errLogPath, nowIso() .. "  " .. tostring(msg) .. "\r\n")
        end)
    end
    -- unescape JSON string body (\\ -> \, \/ -> /) for cfg path values
    local function jsonUnescape(s)
        s = string.gsub(s, "\\\\", "\\")
        s = string.gsub(s, "\\/", "/")
        return s
    end
    local function matchNum(txt, key)
        -- "key": value  (numeric value, e.g. port)
        return string.match(txt, '"' .. key .. '"%s*:%s*(%d+)')
    end
    local function matchTok(txt, key)
        -- "key": "value"  (string value, e.g. token from server.ready)
        local v = string.match(txt, '"' .. key .. '"%s*:%s*"([^"]*)"')
        if v ~= nil then return jsonUnescape(v) end
        return nil
    end

    -- bridge start time: server.ready must be newer than this to be ours.
    local bridgeStart = DateTime.Now

    -- cfg state (nil when missing/unparseable -> degrade, no spawn)
    local cfgPythonExe, cfgRepoRoot, cfgPort = nil, nil, nil
    local cfgOk = false
    do
        local okCfg, cfgErr = pcall(function()
            if not File.Exists(cfgPath) then error("agent.cfg not found: " .. cfgPath) end
            local txt = safestr(File.ReadAllText(cfgPath))
            -- flat JSON, 3 keys, plain string.match (no JSON lib in KopiLua)
            local pe = string.match(txt, '"python_exe"%s*:%s*"([^"]*)"')
            local rr = string.match(txt, '"repo_root"%s*:%s*"([^"]*)"')
            cfgPort  = string.match(txt, '"port"%s*:%s*(%d+)')
            if pe ~= nil then cfgPythonExe = jsonUnescape(pe) end
            if rr ~= nil then cfgRepoRoot  = jsonUnescape(rr) end
            if cfgPythonExe == nil or cfgRepoRoot == nil then
                error("agent.cfg missing python_exe/repo_root")
            end
        end)
        if okCfg then cfgOk = true
        else logError("agent.cfg unusable; server spawn skipped: " .. tostring(cfgErr)) end
    end

    -- agentProc: GLOBAL (no `local`) on purpose -- GC guard for the owned handle
    -- and the only safe pid/HasExited source (the owned handle never throws on a
    -- dead pid, unlike resolving a pid back into a Process by id).
    agentProc = nil
    local lastSpawn = nil
    local agentReady = false
    -- WebView2 task consumption surface: PLAIN LUA GLOBALS (no `local`), same
    -- idiom as agentProc. NEVER stash on the GlobalObj static-type proxy --
    -- writing non-member fields there is build-dependent (throw vs no-op).
    agentSrvReady = false
    agentSrvPort  = nil
    agentSrvToken = nil

    local function spawnServer()
        if not cfgOk then return end
        local okSpawn, spawnErr = pcall(function()
            -- a stale ready from a previous run must not validate the new proc
            if File.Exists(readyPath) then File.Delete(readyPath) end
            local psi = ProcessStartInfo()
            psi.FileName = cfgPythonExe
            -- agentDir provably ends with "\"; strip it so the closing quote is
            -- not escaped ("...\" would escape it on the CreateProcess cmd line).
            psi.Arguments = '-m eud_agent --data-dir "' .. string.sub(agentDir, 1, -2) .. '"'
            psi.UseShellExecute = false
            psi.CreateNoWindow = true
            psi.WorkingDirectory = cfgRepoRoot .. "\\server"
            agentProc = Process.Start(psi)
            -- reset both latches so consumers never read a stale ready/port/token
            -- between server death and the next validation.
            agentReady = false
            agentSrvReady = false
        end)
        lastSpawn = DateTime.Now
        if not okSpawn then logError("server spawn failed: " .. tostring(spawnErr)) end
    end

    -- per-Tick: validate server.ready (owned pid + write time after start).
    local function validateReady()
        if agentProc == nil then return end
        if agentReady then return end
        if not File.Exists(readyPath) then return end
        local okV = pcall(function()
            local txt = safestr(File.ReadAllText(readyPath))
            -- The venv launcher (server\.venv\Scripts\python.exe) re-execs the
            -- base interpreter as a CHILD: the bridge owns the LAUNCHER pid, but
            -- server.ready carries the child pid (server os.getpid). The server
            -- also writes ppid (the launcher), so accept ownership on EITHER.
            -- '"pid"' cannot match inside '"ppid"' (the leading quote precedes a
            -- 'p', not the 'pid' run), so the pid extraction is order-independent
            -- without anchoring; ppid uses its own distinct anchored pattern.
            local pidStr  = string.match(txt, '"pid"%s*:%s*(%d+)')
            local ppidStr = string.match(txt, '"ppid"%s*:%s*(%d+)')
            local ownPid = tostring(agentProc.Id)
            if (pidStr ~= nil and pidStr == ownPid)
                or (ppidStr ~= nil and ppidStr == ownPid) then
                -- write time must be after the bridge started (not a stale file)
                local wt = File.GetLastWriteTime(readyPath)
                if DateTime.Compare(wt, bridgeStart) > 0 then
                    -- expose port+token FIRST, then flip the ready global, then
                    -- the local latch -- so a consumer that sees agentSrvReady
                    -- always finds port+token already populated.
                    agentSrvPort  = matchNum(txt, "port")
                    agentSrvToken = matchTok(txt, "token")
                    agentSrvReady = true
                    agentReady = true
                end
            else
                -- stale ready (neither pid nor ppid matches -- crash leftover):
                -- drop it, respawn recovers
                File.Delete(readyPath)
            end
        end)
        if not okV then logError("server.ready validation error") end
    end

    -- per-Tick: respawn an exited server (throttled to once per 30s).
    local function maybeRespawn()
        if not cfgOk then return end
        if agentProc == nil then return end
        if GlobalObj.pjData == nil then return end
        local exited = false
        pcall(function() exited = agentProc.HasExited end) -- safe on an owned handle
        if not exited then return end
        if lastSpawn ~= nil then
            local elapsed = DateTime.Now:Subtract(lastSpawn).TotalSeconds
            if elapsed < 30 then return end
        end
        spawnServer()
    end

    -- 데이터 에디터: datname → enum(GetDatFileE). None(255) 차단(아니면 KeyNotFound).
    local function resolveDatBinding(datname, param, objId)
        local pj = GlobalObj.pjData
        if pj == nil then return nil, "프로젝트 미로드" end
        local datEnum = pj.Dat:GetDatFileE(datname)
        if string.find(safestr(datEnum), "None") then
            return nil, "invalid datname (units/weapons/flingy/sprites/images/upgrades/techdata/orders)"
        end
        local binding = pj.BindingManager:get_DatBinding(datEnum, param, objId)
        if binding == nil then return nil, "param/index 확인" end
        return binding, nil
    end

    -- ------------------------------------------------------------------
    -- PANEL : WebView2-hosted web panel (replaces the v6 WPF control panel).
    -- The Korean UI lives in the web panel (panel/), not in this Lua source --
    -- the window title stays ASCII. State is kept in PLAIN LUA GLOBALS (no
    -- `local`), same idiom as agentProc/agentSrv* (rules.md "luanet static
    -- proxy" caution): a non-member field on a static-type proxy is build-
    -- dependent. panelWin is the alive-tracking source of truth (re-arm).
    -- ------------------------------------------------------------------
    panelWin       = nil   -- the WebView2-hosting Window (nil => recreate)
    panelView      = nil   -- the WebView2 control
    panelCoreReady = false -- CoreWebView2InitializationCompleted succeeded
    navOk          = false -- last NavigationCompleted IsSuccess
    lastNavAttempt = nil   -- DateTime of the last Navigate (3s backoff source)
    lastNavUrl     = nil   -- URL of the last Navigate (respawn freshness source)
    local NAV_BACKOFF_SECONDS = 3

    local function panelUrl()
        return "http://127.0.0.1:" .. safestr(agentSrvPort) .. "/?token=" .. safestr(agentSrvToken)
    end

    -- Navigate the existing initialized control to the panel URL. Safe to call
    -- repeatedly; records the attempt time + URL (backoff + respawn freshness).
    local function navigatePanel()
        if panelView == nil or not panelCoreReady then return end
        if not agentSrvReady then return end
        local url = panelUrl()
        -- CoreWebView2 is a plain property (dot access; rules.md reserves
        -- get_X() for parameterized properties).
        local okNav = pcall(function()
            panelView.CoreWebView2:Navigate(url)
        end)
        lastNavAttempt = DateTime.Now
        lastNavUrl = url
        if not okNav then
            navOk = false
            logError("panel Navigate failed")
        end
    end

    -- Create the WebView2 window + control. Only when the server is ready (port
    -- and token are populated). Wraps every .NET call so a failure logs and
    -- leaves panelWin nil (the Tick re-arm will retry).
    local function createPanel()
        if not agentSrvReady then return false end
        if panelWin ~= nil then return true end
        local okCreate, cErr = pcall(function()
            panelCoreReady = false
            navOk = false
            local win = Window()
            win.Title = "EUD Agent"
            win.Width = 480; win.Height = 720

            local view = WebView2()
            local cp = CoreWebView2CreationProperties()
            -- explicit user-data-folder under Data\agent (NEVER the default
            -- next-to-exe location; rules.md hard rule).
            cp.UserDataFolder = agentDir .. "webview2"
            view.CreationProperties = cp

            view.CoreWebView2InitializationCompleted:Add(function(s, e)
                local okInit = false
                pcall(function() okInit = e.IsSuccess end)
                if okInit then
                    panelCoreReady = true
                    navigatePanel()
                else
                    panelCoreReady = false
                    logError("CoreWebView2 init failed")
                end
            end)
            view.NavigationCompleted:Add(function(s, e)
                local success = false
                pcall(function() success = e.IsSuccess end)
                if success then
                    navOk = true
                else
                    -- WebView2 never auto-retries: flag for the Tick re-navigate
                    -- (3s backoff via lastNavAttempt).
                    navOk = false
                end
            end)
            win.Closed:Add(function(s, e)
                -- handle-tracking source of truth: a closed window must re-arm.
                panelWin = nil
                panelView = nil
                panelCoreReady = false
            end)

            win.Content = view
            view:EnsureCoreWebView2Async(nil)
            win:Show()
            panelView = view
            panelWin = win
        end)
        if not okCreate then
            logError("createPanel failed: " .. tostring(cErr))
            panelWin = nil
            panelView = nil
            return false
        end
        return true
    end

    -- Show/refocus the panel window (PANEL command); create it if absent + ready.
    local function showPanel()
        if not agentSrvReady then return "ERROR: server not ready" end
        if panelWin == nil then
            if not createPanel() then return "ERROR: panel create failed" end
            return "OK: panel"
        end
        pcall(function() panelWin:Show(); panelWin:Activate() end)
        return "OK: panel"
    end

    -- per-Tick re-arm: recreate the window while "project open AND window not
    -- alive" (the editor closes auxiliary windows on project switch). Also
    -- re-navigate (3s backoff) when a previous navigation failed.
    local function maintainPanel()
        if not agentSrvReady then return end
        if GlobalObj.pjData == nil then return end
        if panelWin == nil then
            createPanel()
            return
        end
        -- window alive: re-navigate when the last nav FAILED, or when the URL
        -- changed (server respawn -> new port/token; a WS disconnect is NOT a
        -- NavigationCompleted failure, so navOk stays true and the panel would
        -- otherwise sit on the dead old-token URL). 3s backoff either way.
        if panelCoreReady and (not navOk or panelUrl() ~= lastNavUrl) then
            local due = true
            if lastNavAttempt ~= nil then
                local elapsed = DateTime.Now:Subtract(lastNavAttempt).TotalSeconds
                if elapsed < NAV_BACKOFF_SECONDS then due = false end
            end
            if due then navigatePanel() end
        end
    end

    -- ------------------------------------------------------------------
    local function handleCommand(cmdText)
        local nl = string.find(cmdText, "\n", 1, true)
        local firstLine, body
        if nl then firstLine = string.sub(cmdText, 1, nl - 1); body = string.sub(cmdText, nl + 1)
        else firstLine = cmdText; body = "" end
        firstLine = string.gsub(firstLine, "\r", "")
        local cmd, arg = string.match(firstLine, "^(%a+)%s*(.-)%s*$")
        if cmd == nil then return "ERROR: 명령 해석 불가" end
        cmd = string.upper(cmd)

        if cmd == "PING" then
            return "PONG " .. tostring(DateTime.Now)
        elseif cmd == "STATUS" then
            local pg = GlobalObj.pgData
            local pj = GlobalObj.pjData
            return "compiling=" .. (pg == nil and "?" or tostring(pg.IsCompilng))
                .. "\r\nproject=" .. (pj ~= nil and ("'" .. safestr(pj.Filename) .. "'") or "(none)")
                .. "\r\nversion=" .. (pg == nil and "?" or tostring(pg.Version))
        elseif cmd == "LIST" then
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local lines = {}
            walk(pj.TEData.PFIles, "", function(p, f)
                local okT, ftype = pcall(function() return f.Filetype end)
                lines[#lines + 1] = p .. "\t" .. (okT and safestr(ftype) or "?")
            end)
            return table.concat(lines, "\r\n")
        elseif cmd == "DUMP" then
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: 프로젝트 미로드" end
            local index = {}
            walk(pj.TEData.PFIles, "", function(p, f)
                local okT, text = pcall(getText, f)
                local flat = string.gsub(p, "[/\\]", "__")
                if okT then File.WriteAllText(outboxDir .. "dump_" .. flat .. ".eps", text)
                    index[#index + 1] = p .. " (len=" .. string.len(text) .. ")"
                else index[#index + 1] = p .. " [ERR]" end
            end)
            File.WriteAllText(outboxDir .. "dump_index.txt", table.concat(index, "\r\n"))
            return "OK: " .. #index .. " files\r\n" .. table.concat(index, "\r\n")
        elseif cmd == "GET" then
            local f = findFile(arg)
            if f == nil then return "ERROR: 파일 없음: '" .. tostring(arg) .. "'" end
            return getText(f)
        elseif cmd == "SET" then
            local f = findFile(arg)
            if f == nil then return "ERROR: 파일 없음: '" .. tostring(arg) .. "'" end
            local okSet, err = pcall(function() f.Scripter.StringText = body end)
            if not okSet then return "ERROR: set 실패(GUI?): " .. tostring(err) end
            local page = f.ParentPage
            if page ~= nil then
                pcall(function() page.NewTextEditor.Text = body end)
                pcall(function() page.OldTextEditor.Text = body end)
            end
            return "OK: set '" .. arg .. "' (" .. string.len(body) .. "B)"
        elseif cmd == "NEWEPS" then
            local name = string.gsub(arg, "^%s*(.-)%s*$", "%1")
            if name == "" or body == "" then
                return "ERROR: usage NEWEPS <name> + body from 2nd line"
            end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            if findFile(name) ~= nil then return "ERROR: duplicate '" .. name .. "'" end
            local okNew, err = pcall(function()
                local nf = TEFile(name, EFileType.CUIEps)
                nf.Scripter.StringText = body
                pj.TEData.PFIles:FileAdd(nf)
                WindowControl.TEOpenFile(nf, 0)
            end)
            if not okNew then return "ERROR: neweps failed: " .. tostring(err) end
            return "OK: neweps '" .. name .. "' (" .. string.len(body) .. "B)"
        elseif cmd == "GETDAT" then
            local a = split(arg, "|")
            if #a < 3 then return "ERROR: 'GETDAT datname|param|objId'" end
            local b, err = resolveDatBinding(a[1], a[2], tonumber(a[3]))
            if b == nil then return "ERROR: " .. err end
            return "OK: " .. a[1] .. "|" .. a[2] .. "|" .. a[3] .. " = " .. safestr(b.Value)
        elseif cmd == "SETDAT" then
            local a = split(arg, "|")
            if #a < 4 then return "ERROR: 'SETDAT datname|param|objId|value'" end
            local b, err = resolveDatBinding(a[1], a[2], tonumber(a[3]))
            if b == nil then return "ERROR: " .. err end
            local before = safestr(b.Value); b.Value = a[4]
            return "OK: " .. a[1] .. "|" .. a[2] .. "|" .. a[3] .. " : '" .. before .. "' -> '" .. safestr(b.Value) .. "'"
        elseif cmd == "PANEL" then
            return showPanel()
        elseif cmd == "BUILD" then
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: 프로젝트 미로드" end
            local okB, err = pcall(function() pj.EudplibData:Build(false) end)
            return okB and "OK: 빌드 시작" or ("ERROR: " .. tostring(err))
        elseif cmd == "LUA" then
            local chunk, lerr = loadstring(body)
            if chunk == nil then return "ERROR: " .. tostring(lerr) end
            local okRun, ret = pcall(chunk)
            return okRun and ("OK: " .. safestr(ret)) or ("ERROR: " .. tostring(ret))
        end
        return "ERROR: 알 수 없는 명령: " .. cmd
    end

    -- ------------------------------------------------------------------
    local timer = DispatcherTimer(DispatcherPriority.Normal)
    timer.Interval = TimeSpan.FromSeconds(1)
    timer.Tick:Add(function(sender, args)
        local okTick, tickErr = pcall(function()
            -- heartbeat: ALWAYS first, unconditional, before the build early-return
            -- (rules.md hard rule; the server self-terminates on >60s staleness).
            pcall(function() File.WriteAllText(agentDir .. "heartbeat.txt", nowIso()) end)
            local pg = GlobalObj.pgData
            if pg ~= nil and pg.IsCompilng then return end
            -- server lifecycle (skipped during builds with the rest of the work)
            validateReady()
            maybeRespawn()
            local pj = GlobalObj.pjData
            File.WriteAllText(agentDir .. "status.txt",
                "time=" .. tostring(DateTime.Now)
                .. "\r\ncompiling=" .. (pg == nil and "?" or tostring(pg.IsCompilng))
                .. "\r\nproject=" .. (pj ~= nil and ("'" .. safestr(pj.Filename) .. "'") or "(none)"))
            -- WebView2 panel re-arm: recreate while "project open AND window not
            -- alive" (project switch closes auxiliary windows) + re-navigate on
            -- a failed nav with a 3s backoff. Handle tracking (panelWin), not a
            -- pjData==nil-only re-arm (rules.md).
            pcall(maintainPanel)
            local files = Directory.GetFiles(inboxDir, "*.cmd")
            for i = 0, files.Length - 1 do
                local cmdPath = tostring(files[i])
                local name = tostring(Path.GetFileNameWithoutExtension(cmdPath))
                local cmdText = safestr(File.ReadAllText(cmdPath))
                local okCmd, result = pcall(handleCommand, cmdText)
                if not okCmd then result = "ERROR: 예외: " .. tostring(result) end
                File.WriteAllText(outboxDir .. name .. ".result", safestr(result))
                File.Delete(cmdPath)
            end
        end)
        if not okTick then
            pcall(function()
                File.AppendAllText(agentDir .. "bridge_error.log", tostring(DateTime.Now) .. "  " .. tostring(tickErr) .. "\r\n")
            end)
        end
    end)
    timer:Start()
    -- spawn the python server once at init (no-op + logged when cfg is unusable)
    spawnServer()
    File.WriteAllText(agentDir .. "bridge_loaded.txt", "agent bridge v7 loaded at " .. tostring(DateTime.Now))
end)

if not ok then error("agent bridge init failed: " .. tostring(initErr)) end
