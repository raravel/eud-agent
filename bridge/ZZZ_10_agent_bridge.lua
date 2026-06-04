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
    local TimeSpan   = luanet.import_type("System.TimeSpan")

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
    -- PANEL : 에이전트 제어판 (4기능, 한글 UI)
    -- ------------------------------------------------------------------
    local panelShown = false
    local fileCounter = 0
    local fileList = {}

    local function showPanel()
        if panelShown then return "패널이 이미 표시됨" end
        local Window     = luanet.import_type("System.Windows.Window")
        local Button     = luanet.import_type("System.Windows.Controls.Button")
        local TextBlock  = luanet.import_type("System.Windows.Controls.TextBlock")
        local TextBox    = luanet.import_type("System.Windows.Controls.TextBox")
        local ListBox    = luanet.import_type("System.Windows.Controls.ListBox")
        local StackPanel = luanet.import_type("System.Windows.Controls.StackPanel")
        local Thickness  = luanet.import_type("System.Windows.Thickness")

        local win = Window()
        win.Title = u8("에이전트 제어판")
        win.Width = 380; win.Height = 580; win.Topmost = true

        local panel = StackPanel()
        panel.Margin = Thickness(12)
        local function add(el) panel.Children:Add(el) end
        local function head(t) local x = TextBlock(); x.Text = u8(t); x.Margin = Thickness(0, 8, 0, 2); add(x) end
        local function mkBtn(label, h)
            local b = Button(); b.Content = u8(label); b.Height = h or 34; b.Margin = Thickness(0, 4, 0, 0); add(b); return b
        end

        head("에이전트 제어판")

        -- (1) 트리거 에디터 열기
        local b1 = mkBtn("1) 트리거 에디터 열기")
        b1.Click:Add(function(s, e)
            local okc, err = pcall(function() WMenus.OpenTriggerEdit() end)
            b1.Content = okc and u8("1) 트리거 에디터 열기 [완료]") or u8("1) 실패")
        end)

        -- (2) 새 eps 파일 생성 + 코드 삽입
        local b2 = mkBtn("2) 새 eps 파일 생성 + 코드 삽입")
        b2.Click:Add(function(s, e)
            local okc, ret = pcall(function()
                local pj = GlobalObj.pjData
                if pj == nil then error("no project") end
                fileCounter = fileCounter + 1
                local fname = "AgentGenerated" .. (fileCounter > 1 and tostring(fileCounter) or "")
                local nf = TEFile(fname, EFileType.CUIEps)
                nf.Scripter.StringText = "// " .. fname .. "\r\nfunction afterTriggerExec() {\r\n    setdeaths(P1, SetTo, 1, \"Terran Marine\");\r\n}\r\n"
                pj.TEData.PFIles:FileAdd(nf)
                WindowControl.TEOpenFile(nf, 0)
                return fname
            end)
            b2.Content = okc and u8("2) 생성됨: " .. tostring(ret)) or u8("2) 실패: " .. tostring(ret))
        end)

        -- (3) 파일 목록 + 선택 열기
        head("파일 목록 (새로고침 후 선택)")
        local list = ListBox(); list.Height = 150; add(list)
        local bR = mkBtn("목록 새로고침")
        local b3 = mkBtn("3) 선택 파일 열기")
        local function refresh()
            list.Items:Clear(); fileList = {}
            local pj = GlobalObj.pjData
            if pj == nil then return end
            walk(pj.TEData.PFIles, "", function(p, f) fileList[#fileList + 1] = f; list.Items:Add(u8(p)) end)
        end
        bR.Click:Add(function(s, e)
            local okc = pcall(refresh)
            bR.Content = okc and u8("목록 새로고침 (" .. #fileList .. ")") or u8("새로고침 실패")
        end)
        b3.Click:Add(function(s, e)
            local okc, ret = pcall(function()
                local idx = list.SelectedIndex
                if idx < 0 then error("목록에서 선택") end
                local f = fileList[idx + 1]
                WindowControl.TEOpenFile(f, 0)
                return safestr(f.FileName)
            end)
            b3.Content = okc and u8("3) 열림: " .. tostring(ret)) or u8("3) " .. tostring(ret))
        end)

        -- (4) 선택 파일 코드 적용
        head("코드 입력 -> 선택 파일에 적용")
        local code = TextBox(); code.Height = 90; code.AcceptsReturn = true
        code.Text = u8("// 에이전트가 입력한 코드\r\nputs(\"hello agent\");")
        add(code)
        local b4 = mkBtn("4) 선택 파일에 코드 적용")
        b4.Click:Add(function(s, e)
            local okc, ret = pcall(function()
                local idx = list.SelectedIndex
                if idx < 0 then error("목록에서 선택") end
                local f = fileList[idx + 1]
                local txt = safestr(code.Text)        -- TextBox 입력(올바른 유니코드)
                f.Scripter.StringText = txt
                local page = f.ParentPage
                if page ~= nil then
                    pcall(function() page.NewTextEditor.Text = txt end)
                    pcall(function() page.OldTextEditor.Text = txt end)
                end
                return safestr(f.FileName) .. " (" .. string.len(txt) .. "B)"
            end)
            b4.Content = okc and u8("4) 적용: " .. tostring(ret)) or u8("4) " .. tostring(ret))
        end)

        win.Content = panel
        win:Show()
        panelShown = true
        return "OK: Agent Panel 표시됨"
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
    local timer = DispatcherTimer()
    timer.Interval = TimeSpan.FromSeconds(1)
    timer.Tick:Add(function(sender, args)
        local okTick, tickErr = pcall(function()
            local pg = GlobalObj.pgData
            if pg ~= nil and pg.IsCompilng then return end
            local pj = GlobalObj.pjData
            File.WriteAllText(agentDir .. "status.txt",
                "time=" .. tostring(DateTime.Now)
                .. "\r\ncompiling=" .. (pg == nil and "?" or tostring(pg.IsCompilng))
                .. "\r\nproject=" .. (pj ~= nil and ("'" .. safestr(pj.Filename) .. "'") or "(none)"))
            -- 프로젝트가 열리면 제어판 자동 표시(1회). 닫히면 재무장 → 다음 프로젝트에서 재표시.
            if pj ~= nil then
                if not panelShown then pcall(showPanel) end
            else
                panelShown = false
            end
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
    File.WriteAllText(agentDir .. "bridge_loaded.txt", "agent bridge v5 loaded at " .. tostring(DateTime.Now))
end)

if not ok then error("agent bridge init failed: " .. tostring(initErr)) end
