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
    -- DatFiles enum carries EVERY dat name (units..orders, portdata, sfxdata, and
    -- the extra/require/button entries). The editor's name-resolver whitelists
    -- only the first 8; we map names to enum OBJECTS directly so portdata/sfxdata
    -- resolve too (rules.md: enum args ALWAYS as imported objects, never ints).
    local DatFiles   = luanet.import_type("EUD_Editor_3.SCDatFiles+DatFiles")
    -- Settings & plugins (B3). TSetting is the program-settings enum nested in
    -- ProgramData (verified ProgramData.vb:174 -- members euddraft/starcraft/
    -- Language). EdsBlockType + EdsBlockItem are nested in BuildData/BuildData.
    -- EdsBlock (verified WriteedsFile.vb:6/99). Enum args ALWAYS as imported
    -- objects (rules.md), so import the enum types here at init.
    local TSetting     = luanet.import_type("EUD_Editor_3.ProgramData+TSetting")
    local EdsBlockType = luanet.import_type("EUD_Editor_3.BuildData+EdsBlockType")
    local EdsBlockItem = luanet.import_type("EUD_Editor_3.BuildData+EdsBlock+EdsBlockItem")
    -- BuildData (B4): EdsFilePath is a SHARED ReadOnly property (BulidPaths.vb --
    -- the file is named BulidPaths.vb but the class is `Partial Public Class
    -- BuildData`; pjData.EudplibData is a BuildData INSTANCE). EDSPATH reads the
    -- Shared property off the imported TYPE proxy (not the instance).
    local BuildData    = luanet.import_type("EUD_Editor_3.BuildData")
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
    -- File-tree helpers (B2). Paths use "/" (the LIST/GET convention), split
    -- into name segments. EFileType + TEFile are imported above.
    -- ------------------------------------------------------------------
    -- ftypeName: the normalized EFileType name string for a node (same idiom as
    -- the LIST branch -- f.FileType is EUD-040, uppercase T). Returns "?" on any
    -- read failure rather than throwing.
    local function ftypeName(f)
        local okT, name = pcall(function()
            return string.match(tostring(f.FileType), "^%s*([%w_]+)")
        end)
        if okT and name then return name end
        return "?"
    end
    -- FileType pre-check (capability-survey row 16): StringText exists ONLY on
    -- CUIScriptEditor (CUIEps/CUIPy) and RawTextScriptEditor (RawText). The
    -- GUIEps/GUIPy (GUIScriptEditor) and ClassicTrigger (ClassicTriggerEditor)
    -- classes have NO StringText member, so assignment THROWS a .NET exception
    -- lua pcall CANNOT catch (critic-refuted "silent no-op"); SCAScript is defunct
    -- (rules.md SCA). This guard is checked BEFORE any StringText assignment and
    -- explicitly rejects the GUI/GUIPy/ClassicTrigger/SCAScript family.
    local function isSettableType(f)
        local t = ftypeName(f)
        if t == "GUIEps" or t == "GUIPy" or t == "ClassicTrigger"
            or t == "SCAScript" then
            return false
        end
        return t == "CUIEps" or t == "CUIPy" or t == "RawText"
    end
    -- isSettableTypeName: same guard keyed on a bare type name string (NEWFILE
    -- validates the requested type before constructing the node).
    local function isSettableTypeName(t)
        if t == "GUIEps" or t == "GUIPy" or t == "ClassicTrigger"
            or t == "SCAScript" then
            return false
        end
        return t == "CUIEps" or t == "CUIPy" or t == "RawText"
    end
    -- creatable type name -> EFileType enum object (whitelist CUIEps/CUIPy/RawText).
    local typeNameToEnum = {
        ["CUIEps"]  = EFileType.CUIEps,
        ["CUIPy"]   = EFileType.CUIPy,
        ["RawText"] = EFileType.RawText,
    }
    -- splitPath: "/"-separated path -> array of non-empty segments.
    local function splitPath(path)
        local segs = {}
        for seg in string.gmatch(path, "([^/]+)") do segs[#segs + 1] = seg end
        return segs
    end
    -- findChildFolder: direct child folder of `parent` by FileName, or nil.
    local function findChildFolder(parent, name)
        for i = 0, parent.FolderCount - 1 do
            local sub = parent:get_Folders(i)
            if safestr(sub.FileName) == name then return sub end
        end
        return nil
    end
    -- findChildFile: direct child file of `parent` by FileName, or nil.
    local function findChildFile(parent, name)
        for i = 0, parent.FileCount - 1 do
            local f = parent:get_Files(i)
            if safestr(f.FileName) == name then return f end
        end
        return nil
    end
    -- findFolder: navigate "/"-segments from the project root to a folder node.
    -- Returns the folder node or nil (an empty path resolves to the root).
    local function findFolder(path)
        local pj = GlobalObj.pjData
        if pj == nil then return nil end
        local node = pj.TEData.PFIles
        local segs = splitPath(path)
        for i = 1, #segs do
            node = findChildFolder(node, segs[i])
            if node == nil then return nil end
        end
        return node
    end
    -- ensureFolder: navigate the "/"-segments, auto-creating missing folders via
    -- parent:FolderAdd(TEFile(name, EFileType.Folder)). Returns the deepest folder
    -- node, or nil,err if a segment collides with an existing FILE of that name.
    local function ensureFolder(path)
        local pj = GlobalObj.pjData
        if pj == nil then return nil, "no project" end
        local node = pj.TEData.PFIles
        local segs = splitPath(path)
        for i = 1, #segs do
            local sub = findChildFolder(node, segs[i])
            if sub == nil then
                if findChildFile(node, segs[i]) ~= nil then
                    return nil, "path segment '" .. segs[i] .. "' is a file"
                end
                local nf = TEFile(segs[i], EFileType.Folder)
                node:FolderAdd(nf)
                sub = nf
            end
            node = sub
        end
        return node, nil
    end
    -- findNode: the file OR folder node at a "/"-path, with its parent folder.
    -- Returns node, parent (parent is nil for the project root). Used by RENAME/
    -- DELFILE/MOVEFILE/SETMAIN. The node's own Parent property is authoritative,
    -- but we resolve the parent by navigation so the root is handled uniformly.
    local function findNode(path)
        local pj = GlobalObj.pjData
        if pj == nil then return nil, nil end
        local segs = splitPath(path)
        if #segs == 0 then return pj.TEData.PFIles, nil end
        local parent = pj.TEData.PFIles
        for i = 1, #segs - 1 do
            parent = findChildFolder(parent, segs[i])
            if parent == nil then return nil, nil end
        end
        local leaf = segs[#segs]
        local file = findChildFile(parent, leaf)
        if file ~= nil then return file, parent end
        local folder = findChildFolder(parent, leaf)
        if folder ~= nil then return folder, parent end
        return nil, nil
    end
    -- isProtectedNode: the top (root) node or the Setting node -- never renamed,
    -- deleted, or moved (verified guard in ProjectExplorer DeleteItem.vb:5:
    -- IsTopFolder Or FileType = EFileType.Setting).
    local function isProtectedNode(f)
        if f == nil then return false end
        local okTop, top = pcall(function() return f.IsTopFolder end)
        if okTop and top then return true end
        return ftypeName(f) == "Setting"
    end
    -- mainFilePath: the "/"-path of the current MainFile node, or "" if unset.
    local function mainFilePath()
        local pj = GlobalObj.pjData
        if pj == nil then return "" end
        local main = pj.TEData.MainFile
        if main == nil then return "" end
        local found = ""
        walk(pj.TEData.PFIles, "", function(p, f) if f == main then found = p end end)
        return found
    end

    -- ------------------------------------------------------------------
    -- Settings & plugins helpers (B3). Two scopes:
    --   project: plain pjData properties (dot get/set), whitelisted below.
    --   program: pgData parameterized property Setting(TSetting) -> from KopiLua
    --            as :get_Setting(enum) / :set_Setting(enum, value) (rules.md:
    --            parameterized properties ALWAYS get_X/set_X). TSetting members
    --            are enum OBJECTS (imported above), never ints/strings.
    -- ANY scope/key outside these whitelists -> ERROR (B3: no theme/UX chrome).
    -- ------------------------------------------------------------------
    -- project-scope key -> getter/setter closures over pjData (dot access). The
    -- whitelist IS the table keys; an unknown key returns nil -> ERROR.
    local projGetters = {
        ["OpenMapName"]  = function(pj) return pj.OpenMapName end,
        ["SaveMapName"]  = function(pj) return pj.SaveMapName end,
        ["AutoBuild"]    = function(pj) return pj.AutoBuild end,
        ["UseCustomtbl"] = function(pj) return pj.UseCustomtbl end,
        ["ViewLog"]      = function(pj) return pj.ViewLog end,
        ["TempFileLoc"]  = function(pj) return pj.TempFileLoc end,
    }
    local projSetters = {
        ["OpenMapName"]  = function(pj, v) pj.OpenMapName = v end,
        ["SaveMapName"]  = function(pj, v) pj.SaveMapName = v end,
        ["AutoBuild"]    = function(pj, v) pj.AutoBuild = v end,
        ["UseCustomtbl"] = function(pj, v) pj.UseCustomtbl = v end,
        ["ViewLog"]      = function(pj, v) pj.ViewLog = v end,
        ["TempFileLoc"]  = function(pj, v) pj.TempFileLoc = v end,
    }
    -- program-scope key -> TSetting enum object. euddraft/starcraft are read/
    -- write; Language is READABLE here but the SETSET branch rejects writes to it
    -- (read-only per B3). No theme/color/UX keys are exposed.
    local progKeyToEnum = {
        ["euddraft"]  = TSetting.euddraft,
        ["starcraft"] = TSetting.starcraft,
        ["Language"]  = TSetting.Language,
    }
    -- program keys the SETSET branch may WRITE (Language excluded -> read-only).
    local progWritable = {
        ["euddraft"]  = true,
        ["starcraft"] = true,
    }

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

    -- Data editor: bridge-local name->enum table over SCDatFiles+DatFiles.
    -- Replaces the editor's 8-name resolver (its whitelist excludes
    -- portdata/sfxdata). Values are enum OBJECTS (rules.md), built once at init.
    -- String keys are the command's dat-name tokens.
    local datNameToEnum = {
        ["units"]    = DatFiles.units,
        ["weapons"]  = DatFiles.weapons,
        ["flingy"]   = DatFiles.flingy,
        ["sprites"]  = DatFiles.sprites,
        ["images"]   = DatFiles.images,
        ["upgrades"] = DatFiles.upgrades,
        ["techdata"] = DatFiles.techdata,
        ["orders"]   = DatFiles.orders,
        ["portdata"] = DatFiles.portdata,
        ["sfxdata"]  = DatFiles.sfxdata,
    }
    -- xdat kinds (ExtraDatBinding key enums) and the require-dat subset.
    local xdatKindToEnum = {
        ["statusinfor"] = DatFiles.statusinfor,
        ["wireframe"]   = DatFiles.wireframe,
        ["ButtonSet"]   = DatFiles.ButtonSet,
    }
    local reqDatToEnum = {
        ["units"]     = DatFiles.units,
        ["upgrades"]  = DatFiles.upgrades,
        ["techdata"]  = DatFiles.techdata,
        ["Stechdata"] = DatFiles.Stechdata,
        ["orders"]    = DatFiles.orders,
    }

    -- Valid GETDAT/SETDAT param names for a dat, for self-correcting errors:
    -- a wrong param name ("Gas Cost", "HitPoints") used to return a bare
    -- "param/index" with no way to discover the editor's display names.
    -- Walk: pj.Dat.GetDatFile(enum).ParameterList -> GetParamname.
    -- GetDatFile is a PARAMETERIZED ReadOnly Property -> :get_GetDatFile(enum)
    -- (rules.md); ParameterList is a plain List(Of CParamater) -> .Count +
    -- :get_Item(i); GetParamname is a parameterless property -> dot access.
    -- DatfileDic holds ALL ten datNameToEnum keys (SCDatFiles.vb:74 loops
    -- 0..9), so the dictionary lookup cannot throw for a whitelisted dat.
    local function datParamNames(datEnum)
        local pj = GlobalObj.pjData
        if pj == nil then return "" end
        local names = {}
        local okWalk = pcall(function()
            local plist = pj.Dat:get_GetDatFile(datEnum).ParameterList
            for i = 0, plist.Count - 1 do
                names[#names + 1] = safestr(plist:get_Item(i).GetParamname)
            end
        end)
        if not okWalk then return "" end
        return table.concat(names, ", ")
    end

    -- Valid GETXDAT/SETXDAT names per kind: a FIXED set, hardcoded from the
    -- editor's ExtraDatBinding dispatch (BindingManager.vb:340-380).
    local xdatNamesByKind = {
        ["statusinfor"] = "Status, Display, Joint",
        ["wireframe"]   = "wire, grp, tran",
        ["ButtonSet"]   = "ButtonSet",
    }

    local function resolveDatBinding(datname, param, objId)
        local pj = GlobalObj.pjData
        if pj == nil then return nil, "no project" end
        local datEnum = datNameToEnum[datname]
        if datEnum == nil then
            return nil, "invalid datname (units/weapons/flingy/sprites/images/upgrades/techdata/orders/portdata/sfxdata)"
        end
        local binding = pj.BindingManager:get_DatBinding(datEnum, param, objId)
        if binding == nil then
            local valid = datParamNames(datEnum)
            local hint = (valid ~= "") and ("; valid params for " .. datname .. ": " .. valid) or ""
            return nil, "param/index (unknown param '" .. tostring(param) .. "' or objId out of range)" .. hint
        end
        return binding, nil
    end

    local function resolveXDatBinding(kind, name, objId)
        local pj = GlobalObj.pjData
        if pj == nil then return nil, "no project" end
        local kindEnum = xdatKindToEnum[kind]
        if kindEnum == nil then
            return nil, "invalid xdat kind (statusinfor/wireframe/ButtonSet)"
        end
        local binding = pj.BindingManager:get_ExtraDatBinding(kindEnum, name, objId)
        if binding == nil then
            return nil, "null binding (name/index out of range); valid names for "
                .. kind .. ": " .. (xdatNamesByKind[kind] or "?")
        end
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
                    -- Citation links (EUD-090): the panel renders evidence links
                    -- as <a target="_blank">, which raises NewWindowRequested.
                    -- Route them to the user's DEFAULT BROWSER (shell-open) and
                    -- mark Handled so WebView2 never spawns its own popup; the
                    -- panel window itself must never navigate away. http(s)
                    -- only; anything else is dropped.
                    pcall(function()
                        view.CoreWebView2.NewWindowRequested:Add(function(s2, e2)
                            local uri = nil
                            pcall(function()
                                e2.Handled = true
                                uri = tostring(e2.Uri)
                            end)
                            if uri ~= nil and (string.sub(uri, 1, 7) == "http://"
                                    or string.sub(uri, 1, 8) == "https://") then
                                pcall(function()
                                    local psi = ProcessStartInfo()
                                    psi.FileName = uri
                                    psi.UseShellExecute = true
                                    Process.Start(psi)
                                end)
                            end
                        end)
                    end)
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
                local okT, ftype = pcall(function()
                    return string.match(tostring(f.FileType), "^%s*([%w_]+)")
                end)
                lines[#lines + 1] = p .. "\t" .. ((okT and ftype) and ftype or "?")
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
            -- FileType pre-check (isSettableType) runs BEFORE the assignment far
            -- below: GUI/GUIPy/ClassicTrigger/SCAScript classes lack the text
            -- setter, so the assignment THROWS uncatchably (capability-survey row
            -- 16). Reject the unsettable family structurally first.
            if not isSettableType(f) then
                return "ERROR: not settable type (" .. ftypeName(f)
                    .. "); only CUIEps/CUIPy/RawText"
            end
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
        elseif cmd == "NEWFILE" then
            -- NEWFILE path|type (+ body). Generalizes NEWEPS: type whitelist
            -- CUIEps/CUIPy/RawText, "/"-path with auto-created parent folders,
            -- duplicate full path -> ERROR.
            local a = split(arg, "|")
            if #a < 2 then return "ERROR: 'NEWFILE path|type' + body from 2nd line" end
            local path = string.gsub(a[1], "^%s*(.-)%s*$", "%1")
            local ftype = string.gsub(a[2], "^%s*(.-)%s*$", "%1")
            if path == "" then return "ERROR: empty path" end
            -- FileType pre-check BEFORE node construction (only settable/creatable
            -- types; GUI/GUIPy/ClassicTrigger/SCAScript rejected).
            if not isSettableTypeName(ftype) then
                return "ERROR: not creatable type (" .. ftype
                    .. "); only CUIEps/CUIPy/RawText"
            end
            local typeEnum = typeNameToEnum[ftype]
            if typeEnum == nil then return "ERROR: unknown type '" .. ftype .. "'" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            if findFile(path) ~= nil then return "ERROR: duplicate '" .. path .. "'" end
            local segs = splitPath(path)
            local leaf = segs[#segs]
            local parentPath = ""
            for i = 1, #segs - 1 do
                if parentPath ~= "" then parentPath = parentPath .. "/" end
                parentPath = parentPath .. segs[i]
            end
            -- ensureFolder auto-creates each missing parent folder via the
            -- editor's parent:FolderAdd(TEFile(name, EFileType.Folder)).
            local parent, ferr = ensureFolder(parentPath)
            if parent == nil then return "ERROR: " .. (ferr or "folder resolve failed") end
            if findChildFile(parent, leaf) ~= nil then
                return "ERROR: duplicate '" .. path .. "'"
            end
            local okNew, err = pcall(function()
                local nf = TEFile(leaf, typeEnum)
                nf.Scripter.StringText = body
                parent:FileAdd(nf)
                WindowControl.TEOpenFile(nf, 0)
            end)
            if not okNew then return "ERROR: newfile failed: " .. tostring(err) end
            return "OK: newfile '" .. path .. "' (" .. ftype .. ", "
                .. string.len(body) .. "B)"
        elseif cmd == "MKDIR" then
            -- MKDIR path. Nested ok via ensureFolder (each segment created with
            -- parent:FolderAdd(TEFile(name, EFileType.Folder))); duplicate -> ERROR.
            local path = string.gsub(arg, "^%s*(.-)%s*$", "%1")
            if path == "" then return "ERROR: empty path" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            if findFolder(path) ~= nil then return "ERROR: duplicate '" .. path .. "'" end
            local node, ferr = ensureFolder(path)
            if node == nil then return "ERROR: " .. (ferr or "mkdir failed") end
            return "OK: mkdir '" .. path .. "'"
        elseif cmd == "RENAME" then
            -- RENAME path (+ newname in BODY). Reject top/Setting, duplicate
            -- sibling. f.FileName = newname then parent FileSort/FolderSort.
            local path = string.gsub(arg, "^%s*(.-)%s*$", "%1")
            local newname = string.gsub(body, "^%s*(.-)%s*$", "%1")
            if path == "" then return "ERROR: empty path" end
            if newname == "" then return "ERROR: empty newname (in body)" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local node, parent = findNode(path)
            if node == nil then return "ERROR: not found '" .. path .. "'" end
            if isProtectedNode(node) then return "ERROR: cannot rename top/Setting node" end
            if parent == nil then return "ERROR: cannot rename root" end
            -- duplicate sibling name (a file or folder already named newname).
            if findChildFile(parent, newname) ~= nil
                or findChildFolder(parent, newname) ~= nil then
                return "ERROR: duplicate sibling '" .. newname .. "'"
            end
            local isFolder = (ftypeName(node) == "Folder")
            local okR, err = pcall(function()
                node.FileName = newname
                if isFolder then parent:FolderSort() else parent:FileSort() end
            end)
            if not okR then return "ERROR: rename failed: " .. tostring(err) end
            return "OK: rename '" .. path .. "' -> '" .. newname .. "'"
        elseif cmd == "DELFILE" then
            -- DELFILE path. Reject top/Setting; clear a dangling MainFile FIRST
            -- (note main-cleared); close any open tab via TECloseTabITem; then
            -- parent FileRemove/FolderRemove + SetDirty (survey row 12).
            local path = string.gsub(arg, "^%s*(.-)%s*$", "%1")
            if path == "" then return "ERROR: empty path" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local node, parent = findNode(path)
            if node == nil then return "ERROR: not found '" .. path .. "'" end
            if isProtectedNode(node) then return "ERROR: cannot delete top/Setting node" end
            if parent == nil then return "ERROR: cannot delete root" end
            local mainCleared = false
            if pj.TEData.MainFile == node then
                pcall(function() pj.TEData.MainFile = nil end)
                mainCleared = true
            end
            -- close an open tab if present (absence tolerated).
            pcall(function() WindowControl.TECloseTabITem(node) end)
            local isFolder = (ftypeName(node) == "Folder")
            local okD, err = pcall(function()
                if isFolder then parent:FolderRemove(node) else parent:FileRemove(node) end
                pj:SetDirty(true)
            end)
            if not okD then return "ERROR: delete failed: " .. tostring(err) end
            local note = mainCleared and " (main-cleared)" or ""
            return "OK: delete '" .. path .. "'" .. note
        elseif cmd == "MOVEFILE" then
            -- MOVEFILE path (+ destFolder in BODY). Locate node + old parent +
            -- dest folder (must exist); reject moving the top/Setting node and
            -- moving into Setting/top-as-target. oldParent FileRemove then dest
            -- FileAdd -- SAME instance, preserving MainFile identity.
            local path = string.gsub(arg, "^%s*(.-)%s*$", "%1")
            local destPath = string.gsub(body, "^%s*(.-)%s*$", "%1")
            if path == "" then return "ERROR: empty path" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local node, parent = findNode(path)
            if node == nil then return "ERROR: not found '" .. path .. "'" end
            if isProtectedNode(node) then return "ERROR: cannot move top/Setting node" end
            if parent == nil then return "ERROR: cannot move root" end
            -- dest folder: an empty body moves to the project root.
            local dest = findFolder(destPath)
            if dest == nil then return "ERROR: dest folder not found '" .. destPath .. "'" end
            if isProtectedNode(dest) and ftypeName(dest) == "Setting" then
                return "ERROR: cannot move into Setting node"
            end
            local isFolder = (ftypeName(node) == "Folder")
            local okM, err = pcall(function()
                if isFolder then
                    parent:FolderRemove(node)
                    dest:FolderAdd(node)
                else
                    parent:FileRemove(node)
                    dest:FileAdd(node)
                end
                pj:SetDirty(true)
            end)
            if not okM then return "ERROR: move failed: " .. tostring(err) end
            return "OK: move '" .. path .. "' -> '" .. destPath .. "/'"
        elseif cmd == "SETMAIN" then
            -- SETMAIN path. node must exist (walk); pj.TEData.MainFile = node.
            local path = string.gsub(arg, "^%s*(.-)%s*$", "%1")
            if path == "" then return "ERROR: empty path" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local node = findFile(path)
            if node == nil then return "ERROR: not found '" .. path .. "'" end
            local okS, err = pcall(function() pj.TEData.MainFile = node end)
            if not okS then return "ERROR: setmain failed: " .. tostring(err) end
            return "OK: main '" .. path .. "'"
        elseif cmd == "GETMAIN" then
            -- GETMAIN (no args). Walk pj.TEData.MainFile to its path via
            -- mainFilePath(); return the current main path or empty string.
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            return mainFilePath()
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
        elseif cmd == "GETXDAT" then
            local a = split(arg, "|")
            if #a < 3 then return "ERROR: 'GETXDAT dat|name|objId'" end
            local b, err = resolveXDatBinding(a[1], a[2], tonumber(a[3]))
            if b == nil then return "ERROR: " .. err end
            return "OK: " .. a[1] .. "|" .. a[2] .. "|" .. a[3] .. " = " .. safestr(b.Value)
        elseif cmd == "SETXDAT" then
            local a = split(arg, "|")
            if #a < 4 then return "ERROR: 'SETXDAT dat|name|objId|value'" end
            local b, err = resolveXDatBinding(a[1], a[2], tonumber(a[3]))
            if b == nil then return "ERROR: " .. err end
            -- Byte-backed setters silently swallow bad values (capability-survey):
            -- assign then RE-READ .Value and return the read-back so the server
            -- can verify the write took.
            local before = safestr(b.Value)
            local okAssign = pcall(function() b.Value = a[4] end)
            if not okAssign then return "ERROR: setxdat assign failed" end
            return "OK: " .. a[1] .. "|" .. a[2] .. "|" .. a[3] .. " : '" .. before .. "' -> '" .. safestr(b.Value) .. "'"
        elseif cmd == "GETTBL" then
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local index = tonumber((string.gsub(arg, "^%s*(.-)%s*$", "%1")))
            if index == nil then return "ERROR: 'GETTBL index'" end
            local b = pj.BindingManager:get_StatTxtBinding(index)
            if b == nil then return "ERROR: null binding (index out of range)" end
            return "OK: " .. index .. " = " .. safestr(b.Value)
        elseif cmd == "SETTBL" then
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local index = tonumber((string.gsub(arg, "^%s*(.-)%s*$", "%1")))
            if index == nil then return "ERROR: 'SETTBL index' + value from 2nd line" end
            local b = pj.BindingManager:get_StatTxtBinding(index)
            if b == nil then return "ERROR: null binding (index out of range)" end
            -- value travels in the BODY (UTF-8 .NET-safe, Korean ok); never the
            -- arg line. body == "NULLSTRING" resets to default (StatNullString).
            local before = safestr(b.Value)
            local okSet = pcall(function()
                if body == "NULLSTRING" then b:DataReset() else b.Value = body end
            end)
            if not okSet then return "ERROR: settbl failed" end
            return "OK: " .. index .. " set (" .. string.len(body) .. "B), was '" .. before .. "'"
        elseif cmd == "RESETDAT" then
            local a = split(arg, "|")
            if #a < 4 then return "ERROR: 'RESETDAT kind|dat|param-or-name|objId'" end
            local kind = a[1]
            local b, err
            if kind == "dat" then
                b, err = resolveDatBinding(a[2], a[3], tonumber(a[4]))
            elseif kind == "xdat" then
                b, err = resolveXDatBinding(a[2], a[3], tonumber(a[4]))
            elseif kind == "tbl" then
                local pj = GlobalObj.pjData
                if pj == nil then return "ERROR: no project" end
                b = pj.BindingManager:get_StatTxtBinding(tonumber(a[4]))
                if b == nil then err = "null binding (index out of range)" end
            else
                return "ERROR: invalid kind (dat/xdat/tbl)"
            end
            if b == nil then return "ERROR: " .. (err or "resolve failed") end
            local okR = pcall(function() b:DataReset() end)
            if not okR then return "ERROR: reset failed" end
            return "OK: reset " .. kind .. " " .. a[2] .. "|" .. a[3] .. "|" .. a[4]
        elseif cmd == "GETREQ" then
            local a = split(arg, "|")
            if #a < 2 then return "ERROR: 'GETREQ dat|objId'" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local datEnum = reqDatToEnum[a[1]]
            if datEnum == nil then return "ERROR: invalid req dat (units/upgrades/techdata/Stechdata/orders)" end
            local objId = tonumber(a[2])
            local cr = pj.ExtraDat:get_RequireData(datEnum)
            if cr == nil then return "ERROR: null require data" end
            local okG, s = pcall(function() return cr:GetCopyString(objId) end)
            if not okG then return "ERROR: getreq failed" end
            return "OK: " .. a[1] .. "|" .. a[2] .. " = " .. safestr(s)
        elseif cmd == "SETREQ" then
            local a = split(arg, "|")
            if #a < 2 then return "ERROR: 'SETREQ dat|objId' + payload from 2nd line" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local datEnum = reqDatToEnum[a[1]]
            if datEnum == nil then return "ERROR: invalid req dat (units/upgrades/techdata/Stechdata/orders)" end
            local objId = tonumber(a[2])
            -- Defense in depth (LUA-channel callers bypass the server's
            -- normalization): the first dot-segment MUST be numeric. The editor's
            -- PasteCopyData coerces it String->Enum (number); a non-numeric first
            -- segment throws an uncatchable InvalidCastException -> editor dialog.
            -- Guard BEFORE any .NET call.
            local firstSeg = string.match(body, "^([^.]*)")
            if firstSeg == nil or not string.match(firstSeg, "^%d+$") then
                return "ERROR: payload first segment must be numeric (0-4)"
            end
            local cr = pj.ExtraDat:get_RequireData(datEnum)
            if cr == nil then return "ERROR: null require data" end
            -- DefaultUse (segment "0") is a SILENT NO-OP in PasteCopyData (no
            -- DefaultUse branch). Route it through the binding's IsDefaultUse
            -- setter instead. All other segments ("1"-"4") go via PasteCopyData.
            if firstSeg == "0" then
                local okD = pcall(function()
                    pj.BindingManager:get_RequireDataBinding(objId, datEnum).IsDefaultUse = true
                end)
                if not okD then return "ERROR: setreq default failed" end
            else
                local okP = pcall(function() cr:PasteCopyData(objId, body) end)
                if not okP then return "ERROR: setreq failed (bad payload?)" end
            end
            local readBack = ""
            pcall(function() readBack = safestr(cr:GetCopyString(objId)) end)
            return "OK: " .. a[1] .. "|" .. a[2] .. " = " .. readBack
        elseif cmd == "GETBTN" then
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local setId = tonumber((string.gsub(arg, "^%s*(.-)%s*$", "%1")))
            if setId == nil then return "ERROR: 'GETBTN setId'" end
            local bs = pj.ExtraDat.ButtonData:get_GetButtonSet(setId)
            if bs == nil then return "ERROR: null button set (id out of range)" end
            local okG, s = pcall(function() return bs:GetCopyString() end)
            if not okG then return "ERROR: getbtn failed" end
            return "OK: " .. setId .. " = " .. safestr(s)
        elseif cmd == "SETBTN" then
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local setId = tonumber((string.gsub(arg, "^%s*(.-)%s*$", "%1")))
            if setId == nil then return "ERROR: 'SETBTN setId' + csv from 2nd line" end
            if body == "" then return "ERROR: empty csv body" end
            -- 8-field CSV pre-check (pos,icon,con,act,conval,actval,enastr,disstr),
            -- one button per dot-separated group; reject malformed before Paste.
            local groups = split(body, ".")
            for gi = 1, #groups do
                local fields = split(groups[gi], ",")
                if #fields < 8 then
                    return "ERROR: malformed csv (button " .. gi .. " needs 8 fields)"
                end
                for fi = 1, 8 do
                    if tonumber(fields[fi]) == nil then
                        return "ERROR: malformed csv (button " .. gi .. " field " .. fi .. " not numeric)"
                    end
                end
            end
            local bs = pj.ExtraDat.ButtonData:get_GetButtonSet(setId)
            if bs == nil then return "ERROR: null button set (id out of range)" end
            local okP = pcall(function() bs:PasteFromString(body) end)
            if not okP then return "ERROR: setbtn failed" end
            -- PasteFromString never clears IsDefault; WriteButtonData.vb skips
            -- Db bytebuffer emission for IsDefault sets, so the runtime patch
            -- table keeps a stale default address with the new button count ->
            -- wild pointer -> SC hard-crash on unit selection (measured
            -- 2026-06-07). Clear it so the edited set gets emitted.
            bs.IsDefault = false
            -- direct mutations don't auto-dirty: mark dirty manually.
            pcall(function() pj:SetDirty(true) end)
            return "OK: setbtn " .. setId .. " (" .. #groups .. " buttons)"
        elseif cmd == "GETSET" then
            -- GETSET scope|key. scope in {project, program}. project keys read
            -- plain pjData props; program keys read pgData:get_Setting(TSetting
            -- enum). Any other scope/key -> ERROR (B3).
            local a = split(arg, "|")
            if #a < 2 then return "ERROR: 'GETSET scope|key'" end
            local scope = string.gsub(a[1], "^%s*(.-)%s*$", "%1")
            local key   = string.gsub(a[2], "^%s*(.-)%s*$", "%1")
            local pj = GlobalObj.pjData
            if scope == "project" then
                if pj == nil then return "ERROR: no project" end
                local getter = projGetters[key]
                if getter == nil then
                    return "ERROR: invalid project key (OpenMapName/SaveMapName/AutoBuild/UseCustomtbl/ViewLog/TempFileLoc)"
                end
                local okG, val = pcall(getter, pj)
                if not okG then return "ERROR: getset failed" end
                return "OK: project|" .. key .. " = " .. safestr(val)
            elseif scope == "program" then
                local pg = GlobalObj.pgData
                if pg == nil then return "ERROR: no pgData" end
                local keyEnum = progKeyToEnum[key]
                if keyEnum == nil then
                    return "ERROR: invalid program key (euddraft/starcraft/Language)"
                end
                local okG, val = pcall(function() return pg:get_Setting(keyEnum) end)
                if not okG then return "ERROR: getset failed" end
                return "OK: program|" .. key .. " = " .. safestr(val)
            end
            return "ERROR: invalid scope (project/program)"
        elseif cmd == "SETSET" then
            -- SETSET scope|key + value in BODY. project keys set plain pjData
            -- props; program keys set pgData:set_Setting(TSetting enum, value)
            -- then flush pgData:SaveSetting(). Language is READ-ONLY -> ERROR.
            -- Any other scope/key -> ERROR (B3: no theme/UX chrome).
            local a = split(arg, "|")
            if #a < 2 then return "ERROR: 'SETSET scope|key' + value from 2nd line" end
            local scope = string.gsub(a[1], "^%s*(.-)%s*$", "%1")
            local key   = string.gsub(a[2], "^%s*(.-)%s*$", "%1")
            local pj = GlobalObj.pjData
            if scope == "project" then
                if pj == nil then return "ERROR: no project" end
                local setter = projSetters[key]
                if setter == nil then
                    return "ERROR: invalid project key (OpenMapName/SaveMapName/AutoBuild/UseCustomtbl/ViewLog/TempFileLoc)"
                end
                local okS = pcall(setter, pj, body)
                if not okS then return "ERROR: setset failed" end
                return "OK: project|" .. key .. " set (" .. string.len(body) .. "B)"
            elseif scope == "program" then
                local pg = GlobalObj.pgData
                if pg == nil then return "ERROR: no pgData" end
                if progKeyToEnum[key] == nil then
                    return "ERROR: invalid program key (euddraft/starcraft/Language)"
                end
                -- Language is read-only (B3): reject the write structurally.
                if not progWritable[key] then
                    return "ERROR: program key '" .. key .. "' is read-only"
                end
                local keyEnum = progKeyToEnum[key]
                local okS = pcall(function()
                    pg:set_Setting(keyEnum, body)
                    pg:SaveSetting()
                end)
                if not okS then return "ERROR: setset failed" end
                return "OK: program|" .. key .. " set (" .. string.len(body) .. "B)"
            end
            return "ERROR: invalid scope (project/program)"
        elseif cmd == "PLUGLIST" then
            -- PLUGLIST (no args). Walk pjData.EdsBlock.Blocks: one line per block
            -- "index TAB BType TAB first-line-of-Texts". Texts are multi-line eds
            -- sections; only the FIRST line is emitted (B3).
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local blocks = pj.EdsBlock.Blocks
            local lines = {}
            for i = 0, blocks.Count - 1 do
                local item = blocks:get_Item(i)
                local btype = safestr(item.BType)
                -- Texts may be nil (built-in blocks); first line only.
                local texts = safestr(item.Texts)
                local nl = string.find(texts, "\n", 1, true)
                local first = nl and string.sub(texts, 1, nl - 1) or texts
                first = string.gsub(first, "\r", "")
                lines[#lines + 1] = i .. "\t" .. btype .. "\t" .. first
            end
            return table.concat(lines, "\r\n")
        elseif cmd == "PLUGADD" then
            -- PLUGADD index + Texts in BODY. Construct an EdsBlockItem of type
            -- UserPlugin, set .Texts = body, insert at index (index=-1 appends).
            -- SetDirty after (direct mutation does not auto-dirty).
            local index = tonumber((string.gsub(arg, "^%s*(.-)%s*$", "%1")))
            if index == nil then return "ERROR: 'PLUGADD index' (index=-1 appends) + Texts from 2nd line" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local blocks = pj.EdsBlock.Blocks
            local at = index
            if at == -1 then at = blocks.Count end
            if at < 0 or at > blocks.Count then
                return "ERROR: index out of range (0.." .. blocks.Count .. " or -1)"
            end
            local okA, err = pcall(function()
                local item = EdsBlockItem(EdsBlockType.UserPlugin)
                item.Texts = body
                blocks:Insert(at, item)
                pj:SetDirty(true)
            end)
            if not okA then return "ERROR: plugadd failed: " .. tostring(err) end
            return "OK: plugadd at " .. at .. " (" .. string.len(body) .. "B)"
        elseif cmd == "PLUGSET" then
            -- PLUGSET index + Texts in BODY. UserPlugin blocks only (built-ins
            -- reject -> ERROR). Blocks:get_Item(i).Texts = body; SetDirty.
            local index = tonumber((string.gsub(arg, "^%s*(.-)%s*$", "%1")))
            if index == nil then return "ERROR: 'PLUGSET index' + Texts from 2nd line" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local blocks = pj.EdsBlock.Blocks
            if index < 0 or index >= blocks.Count then
                return "ERROR: index out of range (0.." .. (blocks.Count - 1) .. ")"
            end
            local item = blocks:get_Item(index)
            if item.BType ~= EdsBlockType.UserPlugin then
                return "ERROR: not a UserPlugin block (built-in, read-only)"
            end
            local okS = pcall(function()
                item.Texts = body
                pj:SetDirty(true)
            end)
            if not okS then return "ERROR: plugset failed" end
            return "OK: plugset " .. index .. " (" .. string.len(body) .. "B)"
        elseif cmd == "PLUGDEL" then
            -- PLUGDEL index. UserPlugin blocks only (built-ins auto-reinsert at
            -- build, so deletion is meaningless -> ERROR). Blocks:RemoveAt(i);
            -- SetDirty.
            local index = tonumber((string.gsub(arg, "^%s*(.-)%s*$", "%1")))
            if index == nil then return "ERROR: 'PLUGDEL index'" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local blocks = pj.EdsBlock.Blocks
            if index < 0 or index >= blocks.Count then
                return "ERROR: index out of range (0.." .. (blocks.Count - 1) .. ")"
            end
            local item = blocks:get_Item(index)
            if item.BType ~= EdsBlockType.UserPlugin then
                return "ERROR: not a UserPlugin block (built-in, cannot delete)"
            end
            local okD = pcall(function()
                blocks:RemoveAt(index)
                pj:SetDirty(true)
            end)
            if not okD then return "ERROR: plugdel failed" end
            return "OK: plugdel " .. index
        elseif cmd == "PLUGMOVE" then
            -- PLUGMOVE from|to. Reorder via RemoveAt(from) + Insert(to, item).
            -- SetDirty. Works for any block (the user reorders the eds output).
            local a = split(arg, "|")
            if #a < 2 then return "ERROR: 'PLUGMOVE from|to'" end
            local fromIdx = tonumber((string.gsub(a[1], "^%s*(.-)%s*$", "%1")))
            local toIdx   = tonumber((string.gsub(a[2], "^%s*(.-)%s*$", "%1")))
            if fromIdx == nil or toIdx == nil then return "ERROR: 'PLUGMOVE from|to' (numeric)" end
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local blocks = pj.EdsBlock.Blocks
            if fromIdx < 0 or fromIdx >= blocks.Count then
                return "ERROR: from index out of range (0.." .. (blocks.Count - 1) .. ")"
            end
            if toIdx < 0 or toIdx >= blocks.Count then
                return "ERROR: to index out of range (0.." .. (blocks.Count - 1) .. ")"
            end
            local okM, err = pcall(function()
                local item = blocks:get_Item(fromIdx)
                blocks:RemoveAt(fromIdx)
                blocks:Insert(toIdx, item)
                pj:SetDirty(true)
            end)
            if not okM then return "ERROR: plugmove failed: " .. tostring(err) end
            return "OK: plugmove " .. fromIdx .. " -> " .. toIdx
        elseif cmd == "PANEL" then
            return showPanel()
        elseif cmd == "BUILD" then
            -- BUILD (B4, hardened): force SCArchive.IsUsed = false (SCA is
            -- defunct -- the dead login modal pops during Build when IsUsed is
            -- true: BulidMain.vb:68-103) and PREFLIGHT the map/euddraft paths
            -- BEFORE Build, so the editor's modal CheckBuildable dialogs
            -- (BulidMain.vb:155-200, missing OpenMapName/SaveMap dir/euddraft exe)
            -- never appear in the headless agent flow. Any missing required path
            -- returns ERROR WITHOUT invoking Build.
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            -- SCArchive.IsUsed = false MUST precede the Build( call (rules.md SCA
            -- rule). The .NET assignment is wrapped (a property setter cannot be
            -- caught by lua pcall once it throws to the Dispatcher, but the
            -- IsUsed setter is a plain field write -- StarCraftArchive.vb:219).
            local okSca = pcall(function() pj.TEData.SCArchive.IsUsed = false end)
            if not okSca then return "ERROR: SCArchive guard failed" end
            -- Preflight: OpenMapName + SaveMapName must be present on disk, and
            -- the euddraft path (program setting) must point at an existing exe.
            -- (CheckBuildable checks the SaveMap DIRECTORY; an empty SaveMapName
            -- has no directory, so reject an empty value too.)
            local openMap = safestr(pj.OpenMapName)
            if openMap == "" or not File.Exists(openMap) then
                return "ERROR: OpenMapName missing or not found"
            end
            local saveMap = safestr(pj.SaveMapName)
            if saveMap == "" then return "ERROR: SaveMapName empty" end
            local saveDir = safestr(Path.GetDirectoryName(saveMap))
            if saveDir == "" or not Directory.Exists(saveDir) then
                return "ERROR: SaveMapName directory not found"
            end
            local pg = GlobalObj.pgData
            if pg == nil then return "ERROR: no pgData" end
            local okE, eudPath = pcall(function()
                return safestr(pg:get_Setting(TSetting.euddraft))
            end)
            if not okE then return "ERROR: euddraft setting read failed" end
            if eudPath == "" or not File.Exists(eudPath) then
                return "ERROR: euddraft path missing or not found"
            end
            local okB, err = pcall(function() pj.EudplibData:Build(false) end)
            return okB and "OK: started" or ("ERROR: " .. tostring(err))
        elseif cmd == "BUILDERR" then
            -- BUILDERR (B4): walk GlobalObj.macro.macroErrorList -- one line per
            -- entry (macro/eps errors accumulated by the last build). `macro` is a
            -- Public field of the GlobalObj module of type MacroManager
            -- (GlobalObj.vb:21); macroErrorList is a List(Of String)
            -- (MacroPluginManager.vb:25), iterated via .Count + :get_Item(i)
            -- (rules.md: List default Item -> get_Item). An empty (non-ERROR)
            -- result = no macro errors recorded.
            local macro = GlobalObj.macro
            if macro == nil then return "ERROR: no macro manager" end
            local list = macro.macroErrorList
            if list == nil then return "" end
            local lines = {}
            for i = 0, list.Count - 1 do
                lines[#lines + 1] = safestr(list:get_Item(i))
            end
            return table.concat(lines, "\r\n")
        elseif cmd == "EDSPATH" then
            -- EDSPATH (B4): return the BuildData-derived temp .eds path
            -- (BuildData.EdsFilePath -- a SHARED ReadOnly property on the imported
            -- TYPE proxy) + pjData.SaveMapName, one per line. Gives the server the
            -- artifact paths for the euddraft re-run fallback + output-map check.
            local pj = GlobalObj.pjData
            if pj == nil then return "ERROR: no project" end
            local edsPath = ""
            local okEds = pcall(function() edsPath = safestr(BuildData.EdsFilePath) end)
            if not okEds then return "ERROR: eds path read failed" end
            return edsPath .. "\r\n" .. safestr(pj.SaveMapName)
        elseif cmd == "LUA" then
            local chunk, lerr = loadstring(body)
            if chunk == nil then return "ERROR: " .. tostring(lerr) end
            local okRun, ret = pcall(chunk)
            return okRun and ("OK: " .. safestr(ret)) or ("ERROR: " .. tostring(ret))
        end
        return "ERROR: 알 수 없는 명령: " .. cmd
    end

    -- ------------------------------------------------------------------
    -- Last idle-Tick project line, reused by the BUSY status write below:
    -- while IsCompilng the Tick must not touch pjData (rules.md), so the
    -- compiling status.txt carries the project string cached on the previous
    -- idle Tick. Plain global (luanet static-proxy idiom, same as panelWin).
    lastProjectLine = "(none)"
    local timer = DispatcherTimer(DispatcherPriority.Normal)
    timer.Interval = TimeSpan.FromSeconds(1)
    timer.Tick:Add(function(sender, args)
        local okTick, tickErr = pcall(function()
            -- heartbeat: ALWAYS first, unconditional, before the build early-return
            -- (rules.md hard rule; the server self-terminates on >60s staleness).
            pcall(function() File.WriteAllText(agentDir .. "heartbeat.txt", nowIso()) end)
            local pg = GlobalObj.pgData
            if pg ~= nil and pg.IsCompilng then
                -- status: ALSO unconditional, BEFORE the build early-return.
                -- status.txt is the server's ONLY busy signal (the 10s->180s
                -- poll-timeout extension + the panel's waiting_build notice);
                -- writing it only on idle Ticks left it permanently
                -- compiling=false, so every command during a build timed out
                -- at 10s with a misleading compiling=False. No pjData access
                -- here (rules.md): the project line is the cached idle value.
                pcall(function()
                    File.WriteAllText(agentDir .. "status.txt",
                        "time=" .. tostring(DateTime.Now)
                        .. "\r\ncompiling=True"
                        .. "\r\nproject=" .. lastProjectLine)
                end)
                return
            end
            -- server lifecycle (skipped during builds with the rest of the work)
            validateReady()
            maybeRespawn()
            local pj = GlobalObj.pjData
            lastProjectLine = (pj ~= nil and ("'" .. safestr(pj.Filename) .. "'") or "(none)")
            File.WriteAllText(agentDir .. "status.txt",
                "time=" .. tostring(DateTime.Now)
                .. "\r\ncompiling=" .. (pg == nil and "?" or tostring(pg.IsCompilng))
                .. "\r\nproject=" .. lastProjectLine)
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
