import { createHash } from "node:crypto";
import { lstat, mkdir, readdir, readFile, rmdir, unlink, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { createConnection } from "node:net";
import { basename, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const args = Object.fromEntries(process.argv.slice(2).map((value, index, all) => value.startsWith("--") ? [value.slice(2), all[index + 1]] : null).filter(Boolean));
const browserPath = resolve(args.browser ?? "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe");
const dist = resolve(args.dist ?? "panel/dist");
const evidence = resolve(args.evidence ?? ".omo/evidence/task-18-e2-project-import-v1-0-2/browser");
const loader = args.loader ?? "C:/Users/ifthe/.codex/plugins/cache/sisyphuslabs/omo/5.1.9/skills/browser/scripts/omowright.mjs";
const viewports = [[960,640],[1280,800],[1920,1080]];
const hosts = ["launcher","setup","settings"];
const inputs = ["mouse","keyboard"];
const scenarios = ["cancelled-source","selected-source","nonempty-destination","missing-map","ambiguous-map","active-scdb","effective-grp","empty-grp","busy-cancel","completed"];
await mkdir(evidence, { recursive: true });
const profiles = viewports.map(([width,height]) => join(evidence,`owned-edge-profile-${width}x${height}`));
for(const profile of profiles) try { await lstat(profile); throw new Error(`owned browser profile already exists: ${profile}`); } catch (error) { if (error.code !== "ENOENT") throw error; }
await writeFile(join(evidence,"ownership-plan.json"), JSON.stringify({ profiles, browserPath, ports: Array.from({length:180},(_,index)=>6200+index), roles:["owned-edge","browser-fixture"] }, null, 2), { flag: "wx" });
const { loadOmowright } = await import(loader);
const { omowright } = await loadOmowright();
const consoleErrors = [];
const results = [];
const children = new Set();
const profileFiles = new Map(), profileDirs = new Set();

async function checkedProfileEntry(root, path) {
  const canonical=resolve(root), candidate=resolve(path);
  if(candidate!==canonical&&!candidate.startsWith(canonical+sep))throw new Error(`profile path escaped: ${candidate}`);
  const parts=candidate===canonical?[]:candidate.slice(canonical.length+1).split(sep);
  let cursor=canonical, info=null;
  for(const part of ["",...parts]){
    if(part)cursor=join(cursor,part);
    try{info=await lstat(cursor);}catch(error){if(error.code==='ENOENT')return null;throw error;}
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
    if(add)profileDirs.add(directory);
    for(const name of await readdir(directory)){
      const path=resolve(directory,name);
      if(!add&&!profileDirs.has(path)&&!profileFiles.has(path))continue;
      const info=await checkedProfileEntry(canonical,path);
      if(!info)continue;
      if(info.isDirectory()){if(add||profileDirs.has(path))queue.push(path);}
      else if(add||profileFiles.has(path))profileFiles.set(path,createHash("sha256").update(await readFile(path)).digest("hex"));
    }
  }
}

async function cleanupProfile(root) {
  const preserved=[];
  for(const [path,hash] of profileFiles){
    if(!path.startsWith(resolve(root)+sep))continue;
    try { const info=await checkedProfileEntry(root,path); if(!info)continue; if(!info.isFile())throw new Error(`registered browser-profile file changed type: ${path}`); const current=createHash("sha256").update(await readFile(path)).digest("hex"); if(current===hash){await checkedProfileEntry(root,path);await unlink(path);}else preserved.push(path); } catch(error) { if(error.code!=="ENOENT")throw error; }
  }
  for(const directory of [...profileDirs].filter(path=>path===resolve(root)||path.startsWith(resolve(root)+sep)).sort((a,b)=>b.length-a.length)){try{const info=await checkedProfileEntry(root,directory);if(!info)continue;if(!info.isDirectory())throw new Error(`registered browser-profile directory changed type: ${directory}`);await rmdir(directory);}catch(error){if(error.code!=="ENOTEMPTY"&&error.code!=="ENOENT")throw error;preserved.push(directory);}}
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
    const fail=error=>{if(settled)return;settled=true;clearTimeout(timer);if(child.exitCode===null)child.kill();children.delete(child);reject(error);};
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
  if (child.exitCode === null) {
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
  const line = tree.split("\n").find(value => value.includes(name) && value.includes("ref="));
  const match = line?.match(/ref=([A-Za-z0-9]+)/);
  if (!match) throw new Error(`accessible element not found: ${name}`);
  return match[1];
}

async function act(page, name, input) {
  const snapshot = await page.snapshot();
  const ref = refFor(snapshot, name);
  if (input === "mouse") await page.locator(ref).click();
  else { await page.locator(ref).focus(); const focused=await page.evaluate(()=>document.activeElement?.textContent??""); if(!focused.includes(name))throw new Error(`keyboard focus did not reach ${name}`); await page.keyboard.press("Enter"); }
}

async function waitForName(page, name, timeoutMs=15000) {
  const deadline=Date.now()+timeoutMs;
  while(Date.now()<deadline){
    try { return refFor(await page.snapshot(),name); } catch { await new Promise(resolveWait=>setTimeout(resolveWait,100)); }
  }
  throw new Error(`accessible state timed out: ${name}`);
}

async function primeHost(page, host, input) {
  if(host==='launcher'){await page.evaluate(()=>window.__TASK18_RESET_TRACE__());return;}
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
  let browser=null;
  try {
    browser = await omowright.connectPipe({ browserPath, browserArgs: ["--headless","--no-first-run",`--window-size=${width},${height}`,`--user-data-dir=${profile}`], storageRoot: profile, dialogPolicy: { accept: true } });
    await registerProfile(profile);
    for (const host of hosts) for (const input of inputs) for (const scenario of scenarios) {
      const port = 6200 + sequence++;
      const directory = join(evidence, `${width}x${height}`, host, input, scenario);
      await mkdir(directory, { recursive: true });
      const fixture = await startFixture(scenario, port);
      let page=null;
      try {
        page = await browser.newTab(`http://127.0.0.1:${port}/?task18Host=${host}&task18Scenario=${scenario}`);
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
        await capture(page, directory, "terminal", width, height);
        if(typeof page.close!=="function") throw new Error("owned page lacks required tab cleanup");
        await page.close();
        await registerProfile(profile,false);
      } catch(error) {
        let observedErrors=[];
        if(page)try{observedErrors=await page.evaluate(()=>window.__TASK18_ERRORS__??[]);}catch{}
        await writeFile(join(directory,"failure.json"),JSON.stringify({scenario,host,input,message:error.message,consoleErrors:observedErrors},null,2));
        throw error;
      } finally { await stopFixture(fixture.child); }
    }
  } finally {
    if(browser){await browser.close();await registerProfile(profile,false);}
    for (const child of [...children]) await stopFixture(child);
    const preserved=await cleanupProfile(profile);
    cleanup.push({ profile:basename(profile), profileExists:preserved.length>0, preserved });
    await writeFile(join(evidence,"cleanup.json"), JSON.stringify({ fixturePids:[...children].map(child=>child.pid), profiles:cleanup }, null, 2));
  }
}
if (consoleErrors.length) throw new Error(`browser console errors: ${JSON.stringify(consoleErrors)}`);
await writeFile(join(evidence,"index.json"), JSON.stringify({ browserPath, dist, profiles:profiles.map(profile=>basename(profile)), results, consoleErrors }, null, 2));
