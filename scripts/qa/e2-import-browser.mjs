import { createHash } from "node:crypto";
import { lstat, mkdir, readdir, readFile, rmdir, unlink, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { createConnection } from "node:net";
import { basename, join, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const args = Object.fromEntries(process.argv.slice(2).map((value, index, all) => value.startsWith("--") ? [value.slice(2), all[index + 1]] : null).filter(Boolean));
const browserPath = resolve(args.browser ?? "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe");
const dist = resolve(args.dist ?? "panel/dist");
const evidence = resolve(args.evidence ?? ".omo/evidence/task-18-e2-project-import-v1-0-2/browser");
const loader = args.loader ?? "C:/Users/ifthe/.codex/plugins/cache/sisyphuslabs/omo/5.1.9/skills/browser/scripts/omowright.mjs";
const profileLifecycle = args["profile-lifecycle"] ?? "host";
const containerLease = args["container-lease"] ?? null;
const container = profileLifecycle === "container";
const focused = args.focused === "true";
if(!["host","container"].includes(profileLifecycle))throw new Error(`invalid profile lifecycle: ${profileLifecycle}`);
if(container){
  if(process.platform!=="linux")throw new Error("container profile lifecycle requires Linux");
  if(!/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(containerLease??""))throw new Error("container lease must be a canonical lowercase UUID");
  if(process.env.TASK18_BROWSER_CONTAINER_LEASE!==containerLease)throw new Error("container lease environment mismatch");
  if(process.env.TASK18_BROWSER_CONTAINER_ROOT!=="/work"||evidence!=="/work/evidence")throw new Error("container work/evidence boundary mismatch");
  if(!browserPath.startsWith("/ms-playwright/"))throw new Error("container browser escaped /ms-playwright");
}
const viewports = [[960,640],[1280,800],[1920,1080]];
const hosts = ["launcher","setup","settings"];
const inputs = ["mouse","keyboard"];
const scenarios = ["cancelled-source","selected-source","nonempty-destination","missing-map","ambiguous-map","active-scdb","effective-grp","empty-grp","busy-cancel","completed"];
const focusedCases = viewportIndex => [
  ...["selected-source","ambiguous-map","active-scdb","busy-cancel","completed"].map(scenario=>({host:"launcher",input:"mouse",scenario})),
  {host:"setup",input:"mouse",scenario:"cancelled-source"},
  {host:"settings",input:"mouse",scenario:"cancelled-source"},
  ...(viewportIndex===1?[{host:"launcher",input:"keyboard",scenario:"ambiguous-map"},{host:"launcher",input:"keyboard",scenario:"busy-cancel"}]:[]),
];
const caseCount = focused ? viewports.reduce((count,_,index)=>count+focusedCases(index).length,0) : viewports.length*hosts.length*inputs.length*scenarios.length;
const profileBase = container ? resolve("/work/profiles",containerLease) : evidence;
if(container&&profileBase!==`/work/profiles/${containerLease}`)throw new Error("container profile boundary mismatch");
await mkdir(evidence, { recursive: true });
if(container)await mkdir(profileBase,{recursive:true});
const profiles = viewports.map(([width,height]) => join(profileBase,`owned-edge-profile-${width}x${height}`));
for(const profile of profiles) try { await lstat(profile); throw new Error(`owned browser profile already exists: ${profile}`); } catch (error) { if (error.code !== "ENOENT") throw error; }
await writeFile(join(evidence,"ownership-plan.json"), JSON.stringify({ profiles, browserPath, profileLifecycle, focused, ...(container?{containerLease}:{}), ports: Array.from({length:caseCount},(_,index)=>6200+index), roles:["owned-edge","browser-fixture"] }, null, 2), { flag: "wx" });
const { loadOmowright } = await import(pathToFileURL(resolve(loader)).href);
const { omowright } = await loadOmowright();
const consoleErrors = [];
const results = [];
const children = new Set();
const profileFiles = new Map(), profileDirs = new Map(), profileRefusals = new Map();

async function checkedProfileEntry(root, path) {
  const canonical=resolve(root), candidate=resolve(path);
  if(candidate!==canonical&&!candidate.startsWith(canonical+sep))throw new Error(`profile path escaped: ${candidate}`);
  const parts=candidate===canonical?[]:candidate.slice(canonical.length+1).split(sep);
  let cursor=canonical, info=null;
  for(const part of ["",...parts]){
    if(part)cursor=join(cursor,part);
    try{info=await lstat(cursor,{bigint:true});}catch(error){if(error.code==='ENOENT')return null;throw error;}
    if(info.isSymbolicLink())throw new Error(`refusing browser-profile reparse: ${cursor}`);
    if(cursor!==candidate&&!info.isDirectory())throw new Error(`browser-profile ancestor is not a directory: ${cursor}`);
  }
  return info;
}

async function registerProfile(root, add=true) {
  const canonical=resolve(root), queue=[canonical];
  while(queue.length){
    const directory=queue.shift();
    const directoryInfo=await checkedProfileEntry(canonical,directory);
    if(!directoryInfo?.isDirectory())throw new Error(`browser-profile directory is missing: ${directory}`);
    const directoryId=`${directoryInfo.dev}:${directoryInfo.ino}:${directoryInfo.birthtimeNs}`;
    if(add)profileDirs.set(directory,directoryId);
    else if(profileDirs.get(directory)!==directoryId){profileRefusals.set(directory,"directory-identity-changed");continue;}
    for(const name of await readdir(directory)){
      const path=resolve(directory,name);
      if(!add&&!profileDirs.has(path)&&!profileFiles.has(path)){if(!profileRefusals.has(path))profileRefusals.set(path,"unknown-after-admission");continue;}
      const info=await checkedProfileEntry(canonical,path);
      if(!info)continue;
      const id=`${info.dev}:${info.ino}:${info.birthtimeNs}`;
      if(info.isDirectory()){
        if(add)queue.push(path);
        else if(!profileDirs.has(path)||profileDirs.get(path)!==id)profileRefusals.set(path,"directory-identity-changed");
        else queue.push(path);
      } else if(!info.isFile())profileRefusals.set(path,"entry-type-changed");
      else if(add){
        try{profileFiles.set(path,{id,initialHash:createHash("sha256").update(await readFile(path)).digest("hex")});}
        catch(error){if(error.code==="EBUSY")profileRefusals.set(path,"locked-at-admission");else throw error;}
      } else if(!profileFiles.has(path)||profileFiles.get(path).id!==id)profileRefusals.set(path,"file-identity-changed");
    }
  }
}

async function cleanupProfile(root) {
  const preserved=[];
  for(const [path,reason] of [...profileRefusals].filter(([candidate])=>candidate===resolve(root)||candidate.startsWith(resolve(root)+sep))){if(await checkedProfileEntry(root,path))preserved.push({path,reason});}
  for(const [path,record] of profileFiles){
    if(!path.startsWith(resolve(root)+sep))continue;
    try {
      const info=await checkedProfileEntry(root,path);if(!info)continue;
      const id=`${info.dev}:${info.ino}:${info.birthtimeNs}`;
      if(!info.isFile()){preserved.push({path,reason:"entry-type-changed"});continue;}
      if(id!==record.id){preserved.push({path,reason:"file-identity-changed"});continue;}
      let current;try{current=createHash("sha256").update(await readFile(path)).digest("hex");}catch(error){if(error.code==="EBUSY"){preserved.push({path,reason:"locked-at-cleanup"});continue;}throw error;}
      if(current!==record.initialHash){preserved.push({path,reason:"bytes-changed"});continue;}
      await checkedProfileEntry(root,path);await unlink(path);
    } catch(error) { if(error.code!=="ENOENT")throw error; }
  }
  for(const [directory,id] of [...profileDirs].filter(([path])=>path===resolve(root)||path.startsWith(resolve(root)+sep)).sort(([a],[b])=>b.length-a.length)){
    try{const info=await checkedProfileEntry(root,directory);if(!info)continue;const currentId=`${info.dev}:${info.ino}:${info.birthtimeNs}`;if(!info.isDirectory()){preserved.push({path:directory,reason:"entry-type-changed"});continue;}if(currentId!==id){preserved.push({path:directory,reason:"directory-identity-changed"});continue;}await rmdir(directory);}
    catch(error){if(error.code!=="ENOTEMPTY"&&error.code!=="ENOENT")throw error;if(error.code==="ENOTEMPTY")preserved.push({path:directory,reason:"directory-not-empty"});}
  }
  return preserved;
}

function portIsFree(port) {
  return new Promise(resolveFree => {
    const socket=createConnection({host:"127.0.0.1",port});
    socket.once("connect",()=>{socket.destroy();resolveFree(false);});
    socket.once("error",()=>resolveFree(true));
  });
}

async function startFixture(scenario, port) {
  if(!(await portIsFree(port))) throw new Error(`fixture port is already owned: ${port}`);
  const file = resolve(fileURLToPath(new URL("./e2-import-browser-fixture.mjs", import.meta.url)));
  const child = spawn(process.execPath, [file,"--dist",dist,"--scenario",scenario,"--port",String(port)], { stdio: ["ignore","pipe","pipe"], windowsHide: true });
  children.add(child);
  return new Promise((resolveReady, reject) => {
    let stderr = "", stdout = "", settled=false;
    const fail=error=>{if(settled)return;settled=true;clearTimeout(timer);if(child.exitCode===null&&child.signalCode===null)child.kill();reject(error);};
    const timer=setTimeout(()=>fail(new Error(`fixture readiness timed out on ${port}: ${stderr}`)),10000);
    child.stderr.on("data", chunk => stderr += chunk);
    child.stdout.on("data", chunk => {
      stdout+=chunk;
      const line=stdout.split(/\r?\n/).find(Boolean);
      if(!line||settled)return;
      try { const ready=JSON.parse(line); if(ready.ready!==true||ready.port!==port||ready.scenario!==scenario)throw new Error("wrong readiness payload"); settled=true;clearTimeout(timer);resolveReady({child,stderr:()=>stderr}); } catch(error) { fail(new Error(`invalid fixture readiness: ${error.message}`)); }
    });
    child.once("exit", code => fail(new Error(`fixture exited ${code}: ${stderr}`)));
  });
}

async function stopFixture(child) {
  if (child.exitCode === null && child.signalCode === null) {
    child.kill();
    await new Promise((resolveExit, reject) => {
      const timer = setTimeout(() => reject(new Error(`fixture PID ${child.pid} did not exit`)), 5000);
      child.once("exit", () => { clearTimeout(timer); resolveExit(); });
    });
  }
  children.delete(child);
}

function refFor(snapshot, name) {
  const tree = typeof snapshot === "string" ? snapshot : (snapshot.tree ?? JSON.stringify(snapshot));
  const refs = typeof snapshot === "object" && snapshot?.refs ? Object.entries(snapshot.refs).filter(([,value]) => value?.role === "button" && value?.name === name) : [];
  if (refs.length === 1) return refs[0][0];
  if (refs.length > 1) throw new Error(`accessible button is not unique: ${name}`);
  const lines = tree.split("\n").filter(value => value.trimStart().startsWith(`- button "${name}" [ref=`));
  const match = lines.length === 1 ? lines[0].match(/ref=([A-Za-z0-9]+)/) : null;
  if (!match) throw new Error(`accessible button not found or not unique: ${name}`);
  return match[1];
}

async function act(page, name, input) {
  const ref = await waitForName(page, name);
  if (input === "mouse") await page.locator(ref).click();
  else {
    const visited = [];
    const inspectFocus = async () => {
      const focused = await page.evaluate(() => {
        const active = document.activeElement;
        return {
          tag: active?.tagName ?? "",
          ariaLabel: active?.getAttribute("aria-label") ?? "",
          visibleText: active instanceof HTMLElement ? active.innerText.trim() : "",
        };
      });
      visited.push(`${focused.tag} aria-label=${focused.ariaLabel} text=${focused.visibleText}`.slice(0, 120));
      return focused;
    };
    let focused = await inspectFocus();
    for (let index = 0; index <= 32; index += 1) {
      if (focused.tag === "BUTTON" && (focused.ariaLabel.includes(name) || focused.visibleText.includes(name))) {
        await page.keyboard.press("Enter");
        return;
      }
      if (index === 32) break;
      await page.keyboard.press("Tab");
      focused = await inspectFocus();
    }
    throw new Error(`keyboard focus did not reach ${name}: ${visited.join(" -> ")}`);
  }
}

async function waitForName(page, name, timeoutMs=15000) {
  const deadline=Date.now()+timeoutMs;
  let attempts=0,snapshotSuccesses=0,lastSnapshotError=null,lastLookupError=null;
  while(Date.now()<deadline){
    attempts++;
    try {
      const snapshot=await page.snapshot();
      snapshotSuccesses++;
      try { return refFor(snapshot,name); } catch(error) { lastLookupError=diagnosticText(error); }
    } catch(error) { lastSnapshotError=diagnosticText(error); }
    await new Promise(resolveWait=>setTimeout(resolveWait,100));
  }
  throw new Error(`accessible state timed out: ${name}; attempts=${attempts} snapshotSuccesses=${snapshotSuccesses} lastSnapshotError=${lastSnapshotError} lastLookupError=${lastLookupError}`);
}

const diagnosticText=value=>String(value).slice(0,2048);

async function readOwnedChromiumProcesses() {
  const receipt={available:false,error:null,scanCount:0,resultCount:0,scanCap:4096,resultCap:128,scanTruncated:false,resultTruncated:false,entryErrors:0,rows:[]};
  if(!container){receipt.error="not-container";return receipt;}
  let names;
  try { names=(await readdir("/proc")).filter(value=>/^\d+$/.test(value)); }
  catch(error){receipt.error=diagnosticText(error);return receipt;}
  receipt.available=true;
  receipt.scanTruncated=names.length>receipt.scanCap;
  for(const name of names.slice(0,receipt.scanCap)){
    receipt.scanCount++;
    try {
      const status=await readFile(`/proc/${name}/status`,"utf8");
      const field=key=>status.match(new RegExp(`^${key}:\\s*(.+)$`,"m"))?.[1]??null;
      const processName=field("Name");
      if(/^(?:chrome|chromium)/.test(processName??""))receipt.rows.push({pid:Number(name),name:processName,state:field("State"),ppid:Number(field("PPid")),nspid:field("NSpid")});
    } catch { receipt.entryErrors++; }
    if(receipt.rows.length===receipt.resultCap){receipt.resultTruncated=true;break;}
  }
  receipt.resultCount=receipt.rows.length;
  return receipt;
}

async function captureFailure(page, directory) {
  const domExpression=`(()=>{const root=document.getElementById("root");const resources=performance.getEntriesByType("resource").filter(({initiatorType,name})=>["script","link"].includes(initiatorType)&&/\\.(?:js|css)(?:[?#]|$)/.test(name)).slice(-50).map(({name,initiatorType,duration,transferSize,decodedBodySize})=>({name,initiatorType,duration,transferSize,decodedBodySize}));return{readyState:document.readyState,url:location.href,outerHTML:document.documentElement?.outerHTML??"",bodyText:document.body?.textContent??"",root:{children:root?.childElementCount??null,htmlLength:root?.innerHTML.length??null},modules:[...document.scripts].filter(script=>script.type==="module").map(script=>({src:script.src,type:script.type})),resources,trace:window.__TASK18_TRACE__??null,errors:window.__TASK18_ERRORS__??[]};})()`;
  const invoke=operation=>Promise.resolve().then(operation);
  const [snapshotResult,domResult,pngResult]=await Promise.allSettled([
    invoke(()=>page.snapshot()),
    invoke(()=>page.evaluate(domExpression)),
    invoke(()=>page.screenshot()),
  ]);
  const receipt={channels:{}},writes=[];
  const rejected=(channel,reason)=>{receipt.channels[channel]={status:"rejected",error:diagnosticText(reason)};};
  const jsonChannel=(channel,file,result)=>{
    if(result.status==="rejected"){rejected(channel,result.reason);return;}
    try { writes.push({channel,file,data:JSON.stringify(result.value,null,2)}); receipt.channels[channel]={status:"fulfilled",file}; }
    catch(error){rejected(channel,error);}
  };
  jsonChannel("snapshot","failure-snapshot.json",snapshotResult);
  jsonChannel("dom","failure-dom.json",domResult);
  if(pngResult.status==="rejected")rejected("png",pngResult.reason);
  else {
    const png=pngResult.value;
    if(!Buffer.isBuffer(png)||png.length<24||png.subarray(0,8).toString("hex")!=="89504e470d0a1a0a")rejected("png","invalid failure PNG capture");
    else {
      const metadata={status:"fulfilled",file:"failure.png",signature:png.subarray(0,8).toString("hex"),width:png.readUInt32BE(16),height:png.readUInt32BE(20),bytes:png.length,sha256:createHash("sha256").update(png).digest("hex")};
      receipt.channels.png=metadata;
      writes.push({channel:"png",file:metadata.file,data:png});
    }
  }
  const writeResults=await Promise.allSettled(writes.map(({file,data})=>writeFile(join(directory,file),data,{flag:"wx"})));
  writeResults.forEach((result,index)=>{if(result.status==="rejected")receipt.channels[writes[index].channel].writeError=diagnosticText(result.reason);});
  return receipt;
}

async function primeHost(page, host, input) {
  if(host==='launcher'||host==='setup'){await page.evaluate(()=>window.__TASK18_RESET_TRACE__());return;}
  await page.evaluate(()=>window.__TASK18_SET_PRIMING__(true));
  await act(page,"E2 프로젝트 가져오기",input);
  await act(page,"E2S/E2P 선택",input);
  await act(page,"새 작업 폴더 선택",input);
  await act(page,"선택한 위치로 가져오기",input);
  await waitForName(page,host==='setup'?"E2 프로젝트 가져오기":"설정 열기");
  await page.evaluate(()=>window.__TASK18_SET_PRIMING__(false));
  await page.evaluate(()=>window.__TASK18_RESET_TRACE__());
}

async function capture(page, directory, id, width, height) {
  const raw = await page.snapshot();
  const tree = typeof raw === "string" ? raw : (raw.tree ?? JSON.stringify(raw));
  const dom = await page.evaluate(() => ({
    active: document.activeElement?.textContent?.trim() ?? "",
    viewport: { width: innerWidth, height: innerHeight },
    body: { scrollWidth: document.body.scrollWidth, clientWidth: document.body.clientWidth },
    dialogs: [...document.querySelectorAll('[role="dialog"]')].map(element => {
      const before=element.scrollTop;element.scrollTop=element.scrollHeight;const reached=element.scrollHeight<=element.clientHeight||element.scrollTop>before;element.scrollTop=before;
      return { text: element.textContent, rect: element.getBoundingClientRect().toJSON(), scrollHeight: element.scrollHeight, clientHeight: element.clientHeight, scrollWidth:element.scrollWidth, clientWidth:element.clientWidth, reached };
    }),
  }));
  if (dom.viewport.width !== width || dom.viewport.height !== height) throw new Error(`viewport mismatch in ${id}: ${dom.viewport.width}x${dom.viewport.height}`);
  if (dom.body.scrollWidth > dom.body.clientWidth) throw new Error(`horizontal overflow in ${id}`);
  for(const dialog of dom.dialogs) if(dialog.scrollWidth>dialog.clientWidth||!dialog.reached||dialog.rect.left<0||dialog.rect.right>width) throw new Error(`dialog is not keyboard/scroll reachable in ${id}`);
  const captured = await page.screenshot();
  const png = Buffer.isBuffer(captured) ? captured : Buffer.from(captured.buffer ?? captured);
  if (!Buffer.isBuffer(png) || png.subarray(0,8).toString("hex") !== "89504e470d0a1a0a") throw new Error(`invalid PNG capture in ${id}`);
  const pngWidth=png.readUInt32BE(16), pngHeight=png.readUInt32BE(20);
  if(pngWidth!==width||pngHeight!==height) throw new Error(`PNG dimensions mismatch in ${id}: ${pngWidth}x${pngHeight}`);
  const pngPath = join(directory, `${id}.png`);
  await Promise.all([writeFile(pngPath, png), writeFile(join(directory, `${id}-a11y.txt`), tree), writeFile(join(directory, `${id}-dom.json`), JSON.stringify(dom,null,2))]);
  results.push({ id, width, height, png: pngPath, pngSha256:createHash("sha256").update(png).digest("hex"), a11y: `${id}-a11y.txt`, dom: `${id}-dom.json` });
}

async function assertScenario(page, scenario) {
  const expected={"cancelled-source":"E2S/E2P 선택","selected-source":"C:/task18/package/project.e2s","nonempty-destination":"비어 있지 않습니다","missing-map":"원본 맵 다시 연결","active-scdb":"SCDB","effective-grp":"GRP","busy-cancel":"가져오기를 취소했습니다."}[scenario];
  const closes=["ambiguous-map","empty-grp","completed"].includes(scenario), deadline=Date.now()+15000;
  while(Date.now()<deadline){
    const state=await page.evaluate(()=>({text:document.body.textContent??"",dialog:Boolean(document.querySelector('[role="dialog"]')),errors:window.__TASK18_ERRORS__??[],trace:window.__TASK18_TRACE__}));
    if(state.errors.length) throw new Error(`page errors in ${scenario}: ${state.errors.join(" | ")}`);
    if((!expected||state.text.includes(expected))&&(!closes||!state.dialog)) {
      const commands=state.trace.commands;
      if(scenario==='cancelled-source'&&commands.includes('setup_pick_import_destination')) throw new Error('cancelled source advanced to destination');
      if(scenario==='selected-source'&&commands.includes('setup_pick_import_destination')) throw new Error('selected-source advanced past source selection');
      if(scenario==='ambiguous-map'&&(state.trace.preflights[0]?.state!=='needsRelink'||state.trace.preflights[0].needs[0].candidates.length!==2||!state.trace.relink)) throw new Error('ambiguous map did not complete fingerprint-bound manual relink');
      if(scenario==='empty-grp'&&state.trace.preflights.at(-1)?.features.length!==0) throw new Error('true-empty GRP was advertised as active');
      if(scenario==='busy-cancel'&&(!commands.includes('setup_import_e2')||!commands.includes('setup_cancel_e2_import'))) throw new Error('busy cancellation did not traverse import and cancel commands');
      if(["ambiguous-map","empty-grp","completed"].includes(scenario)&&!commands.includes('setup_import_e2')) throw new Error(`${scenario} did not invoke typed import`);
      return;
    }
    await new Promise(resolveWait=>setTimeout(resolveWait,100));
  }
  throw new Error(`${scenario} did not reach ${closes?'completed adoption':expected}`);
}

let sequence = 0;
const cleanup = [];
for (let viewportIndex=0;viewportIndex<viewports.length;viewportIndex++) {
  const [width,height]=viewports[viewportIndex], profile=profiles[viewportIndex];
  await mkdir(profile);
  await writeFile(join(profile,"task18-owned.json"),JSON.stringify({profile,width,height}),{flag:"wx"});
  let browser=null,browserPrimaryError=null,browserCloseError=null,browserProcess=null;
  try {
    browser = await omowright.connectPipe({ browserPath, browserArgs: ["--headless","--no-first-run",...(container?["--no-sandbox"]:[]),`--window-size=${width},${height}`,`--user-data-dir=${profile}`], storageRoot: profile, dialogPolicy: { accept: true } });
    browserProcess=browser.browserProcess;
    if(!container)await registerProfile(profile);
    const cases=focused?focusedCases(viewportIndex):hosts.flatMap(host=>inputs.flatMap(input=>scenarios.map(scenario=>({host,input,scenario}))));
    const focusedPage=focused?await browser.newTab():null;
    if(focused)await omowright.repinViewport(focusedPage,{width,height});
    for (const {host,input,scenario} of cases) {
      const port = 6200 + sequence++;
      const directory = join(evidence, `${width}x${height}`, host, input, scenario);
      await mkdir(directory, { recursive: true });
      const fixture = await startFixture(scenario, port);
      let page=null,primaryError=null;
      try {
        const fixtureUrl=`http://127.0.0.1:${port}/?task18Host=${host}&task18Scenario=${scenario}`;
        page = focusedPage??await browser.newTab();
        await omowright.repinViewport(page,{width,height});
        await page.goto(fixtureUrl);
        const navigationDeadline=Date.now()+15000;
        let navigationState=null,lastNavigationError=null,navigationErrorCount=0;
        while(Date.now()<navigationDeadline){
          try {
            navigationState=await page.evaluate(()=>({url:location.href,resetType:typeof window.__TASK18_RESET_TRACE__}));
            if(navigationState.url===fixtureUrl&&navigationState.resetType==="function")break;
          } catch(error) { lastNavigationError=String(error); navigationErrorCount++; }
          await new Promise(resolveWait=>setTimeout(resolveWait,100));
        }
        if(navigationState?.url!==fixtureUrl||navigationState?.resetType!=="function")throw new Error(`fixture navigation did not expose bridge: state=${JSON.stringify(navigationState)} evaluationErrors=${navigationErrorCount} last=${lastNavigationError}`);
        await waitForName(page,"E2 프로젝트 가져오기");
        if(!(await page.evaluate(()=>window.__TASK18_TRACE__.commands.includes("app_settings")&&!document.body.textContent?.includes("앱 설정을 불러오지 못했습니다."))))
          throw new Error("production appSettingsGet transport or settings DTO failed");
        await omowright.repinViewport(page,{width,height});
        const navigationReceipt=await page.evaluate(()=>({url:location.href,resetType:typeof window.__TASK18_RESET_TRACE__,innerWidth,innerHeight}));
        if(navigationReceipt.url!==fixtureUrl||navigationReceipt.resetType!=="function"||navigationReceipt.innerWidth!==width||navigationReceipt.innerHeight!==height)throw new Error(`fixture navigation receipt mismatch: ${JSON.stringify(navigationReceipt)}`);
        process.stdout.write(JSON.stringify({task18Navigation:{...navigationReceipt,navigationErrorCount,lastNavigationError}})+"\n");
        await primeHost(page,host,input);
        if (host === "settings") { await act(page, "설정 열기", input); await act(page, "프로젝트", input); }
        await act(page, "E2 프로젝트 가져오기", input);
        await capture(page, directory, "dialog", width, height);
        await act(page, "E2S/E2P 선택", input);
        if (!['cancelled-source','selected-source'].includes(scenario)) await act(page, "새 작업 폴더 선택", input);
        if (scenario === "ambiguous-map") { await waitForName(page,"원본 맵 다시 연결"); await capture(page,directory,"needs-relink",width,height); await act(page,"원본 맵 다시 연결",input); await waitForName(page,"선택한 위치로 가져오기"); }
        if (["ambiguous-map","empty-grp","busy-cancel","completed"].includes(scenario)) await act(page, "선택한 위치로 가져오기", input);
        if (scenario === "busy-cancel") { await waitForName(page,"가져오기 취소"); await act(page, "가져오기 취소", input); }
        await assertScenario(page,scenario);
        if(host==="setup"&&scenario==="completed"&&await page.evaluate(()=>document.body.textContent?.includes("eud-agent 시작 설정")))
          throw new Error("completed setup import remained in SetupScreen");
        await capture(page, directory, "terminal", width, height);
        if(!focused){
          if(typeof page.close!=="function") throw new Error("owned page lacks required tab cleanup");
          await page.close();page=null;
        }
        if(!container)await registerProfile(profile,false);
      } catch(error) {
        primaryError=error;
        let observedErrors=[],diagnostic=null,diagnosticError=null;
        if(page)try{diagnostic=await captureFailure(page,directory);}catch(captureError){diagnosticError=String(captureError);}
        if(page)try{observedErrors=await page.evaluate(()=>window.__TASK18_ERRORS__??[]);}catch{}
        const ownedChromiumProcesses=await readOwnedChromiumProcesses();
        const ownedBrowserProcess=browserProcess?{pid:browserProcess.pid,exitCode:browserProcess.exitCode,signalCode:browserProcess.signalCode}:null;
        await writeFile(join(directory,"failure.json"),JSON.stringify({scenario,host,input,message:error.message,consoleErrors:observedErrors,diagnostic,diagnosticError:diagnosticError===null?null:diagnosticText(diagnosticError),ownedBrowserProcess,ownedChromiumProcesses},null,2));
        throw error;
      } finally {
        const cleanupErrors=[];
        if(page&&!focused)try{await page.close();page=null;}catch(error){cleanupErrors.push(error);}
        try{await stopFixture(fixture.child);}catch(error){cleanupErrors.push(error);}
        if(cleanupErrors.length){
          const cleanupError=new AggregateError(cleanupErrors,"scenario cleanup failed");
          if(!primaryError)throw cleanupError;
          console.error(`secondary scenario cleanup failures: ${cleanupErrors.map(String).join(" | ")}`);
        }
      }
    }
  } catch(error) {
    browserPrimaryError=error;
    throw error;
  } finally {
    const browserProcessBeforeClose=browserProcess?{pid:browserProcess.pid,exitCode:browserProcess.exitCode,signalCode:browserProcess.signalCode}:null;
    if(browser){
      try{await browser.close();}catch(error){browserCloseError=diagnosticText(error);}
      if(!container)await registerProfile(profile,false);
    }
    const browserProcessAfterClose=browserProcess?{pid:browserProcess.pid,exitCode:browserProcess.exitCode,signalCode:browserProcess.signalCode}:null;
    for (const child of [...children]) await stopFixture(child);
    if(container)cleanup.push({ profile:basename(profile), profileExists:true, preserved:[], custody:`relinquishedToContainerLease:${containerLease}`, cleanupPass:false, externalExactContainerRemovalRequired:true });
    else { const preserved=await cleanupProfile(profile); cleanup.push({ profile:basename(profile), profileExists:Boolean(await checkedProfileEntry(profile,profile)), preserved }); }
    await writeFile(join(evidence,"cleanup.json"), JSON.stringify({ fixturePids:[...children].map(child=>child.pid), browserProcessBeforeClose, browserProcessAfterClose, browserCloseError, profiles:cleanup }, null, 2));
    if(browserCloseError){
      if(browserPrimaryError)console.error(`secondary browser close failure: ${browserCloseError}`);
      else throw new Error(`browser close failed: ${browserCloseError}`);
    }
  }
}
if (consoleErrors.length) throw new Error(`browser console errors: ${JSON.stringify(consoleErrors)}`);
await writeFile(join(evidence,"index.json"), JSON.stringify({ browserPath, dist, profileLifecycle, focused, ...(container?{containerLease}:{}), profiles:profiles.map(profile=>basename(profile)), results, consoleErrors }, null, 2));
if(!container&&cleanup.some(entry=>entry.profileExists))throw new Error("browser profile cleanup refused; see cleanup.json");
