import { createServer } from "node:http";
import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize, resolve, sep } from "node:path";

const scenarios = new Set([
  "cancelled-source", "selected-source", "nonempty-destination", "missing-map",
  "ambiguous-map", "active-scdb", "effective-grp", "empty-grp", "busy-cancel", "completed",
]);
const args = Object.fromEntries(process.argv.slice(2).map((value, index, all) => value.startsWith("--") ? [value.slice(2), all[index + 1]] : null).filter(Boolean));
const dist = resolve(args.dist ?? "panel/dist");
const port = Number(args.port ?? 6175);
const scenario = args.scenario ?? "selected-source";
if (!scenarios.has(scenario) || !Number.isInteger(port)) throw new Error("invalid browser fixture arguments");
await stat(join(dist, "index.html"));

const issue = (code, message, feature) => ({ code, severity: "fatal", message, ...(feature ? { feature } : {}) });
const missingNeed = { id: "ProjectSET:InputMap", kind: "inputMap", label: "scene.scx", originalPath: "Missing/scene.scx", candidates: [], extensions: ["scx", "scm"] };
const ambiguousNeed = { ...missingNeed, candidates: ["C:/task18/package/Map/scene.scx", "C:/task18/package/scene.scx"] };
const fingerprint = "b".repeat(64);

function preflight() {
  if (scenario === "missing-map") return { state: "needsRelink", fingerprint, needs: [missingNeed] };
  if (scenario === "ambiguous-map") return { state: "needsRelink", fingerprint, needs: [ambiguousNeed] };
  if (scenario === "active-scdb") return { state: "fatal", fingerprint, issues: [issue("active_scdb_unsupported", "활성 SCDB 기능은 가져올 수 없습니다.", "scdb")] };
  if (scenario === "effective-grp") return { state: "fatal", fingerprint, issues: [issue("active_grp_unsupported", "활성 GRP 교체는 가져올 수 없습니다.", "grp")] };
  return { state: "ready", fingerprint, sourceKind: scenario === "ambiguous-map" ? "e2p" : "e2s", features: scenario === "empty-grp" ? [] : ["structGui"], warnings: [] };
}

function bridgeSource() {
  return `<script>(()=>{
const scenario=${JSON.stringify(scenario)}, host=new URLSearchParams(location.search).get('task18Host')??'launcher', fp=${JSON.stringify(fingerprint)}, preflight=${JSON.stringify(preflight())};
let next=1, listeners=[], importId='browser-import-1', opened=false, priming=false, relinked=false;
const trace={commands:[],preflights:[],relink:null};
const callbacks=new Map();
const result=(state,extra={})=>({state,importId,fingerprint:fp,...extra});
const emit=(payload)=>listeners.filter(item=>item.event==='e2_import').forEach(item=>window['_' + item.handler]?.({event:'e2_import',id:item.eventId,payload:{type:'e2_import',result:payload}}));
const invoke=async(cmd,args={})=>{
  trace.commands.push(cmd);
  if(cmd==='setup_pick_e2_source') return !priming&&scenario==='cancelled-source'?null:'C:/task18/package/project.e2s';
  if(cmd==='setup_pick_import_destination') return {path:'C:/task18/destination',empty:priming||scenario!=='nonempty-destination'};
  if(cmd==='setup_pick_e2_relink'){relinked=true;trace.relink='C:/task18/package/Map/scene.scx';return trace.relink;}
  if(cmd==='setup_e2_preflight'){const value=priming||relinked?{state:'ready',fingerprint:fp,sourceKind:scenario==='ambiguous-map'?'e2p':'e2s',features:scenario==='empty-grp'?[]:['structGui'],warnings:[]}:preflight;trace.preflights.push(value);return value;}
  if(cmd==='setup_cancel_e2_import'){const value=result('cancelled');emit(value);return value;}
  if(cmd==='setup_import_e2'){
    const progress=result('progress',{phase:'materializing',completed:2,total:5,message:'리소스를 복사하는 중'});emit(progress);
    if(scenario==='busy-cancel'&&!priming) return progress;
    const done=result('completed',{projectOpened:true,projectPath:'C:/task18/destination'});opened=true;queueMicrotask(()=>emit(done));return done;
  }
  if(cmd==='plugin:event|listen'){const eventId=next++;listeners.push({eventId,event:args.event,handler:args.handler});return eventId;}
  if(cmd==='plugin:event|unlisten'){listeners=listeners.filter(item=>item.eventId!==args.eventId);return null;}
  if(cmd==='setup_status'){
    const setup=opened&&host==='setup';
    const providers=['codex','claude-code','antigravity','opencode-go','ollama'].map(provider=>({provider,availability:'ready',selectedAsDefault:provider==='ollama',canInstall:false,canImport:false,experimental:false,detailCode:null}));
    return {projectPath:opened?'C:/task18/destination':'',projectValid:opened,projectOpened:false,euddraftPath:setup?'':'C:/task18/euddraft.exe',euddraftValid:!setup,assetsReady:true,defaultProvider:'ollama',providers,setupRequired:setup,error:null};
  }
  if(cmd==='project_recent_list') return [];
  if(cmd==='project_list') return [];
  if(cmd==='provider_status') return [];
  if(cmd==='app_settings_get') return {};
  if(cmd.includes('version')||cmd.includes('status')) return null;
  return null;
};
window.__TAURI_INTERNALS__={
  invoke,
  transformCallback(cb,once=false){const id=next++;callbacks.set(id,cb);window['_'+id]=(value)=>{cb(value);if(once){callbacks.delete(id);delete window['_'+id];}};return id;},
  unregisterCallback(id){callbacks.delete(id);delete window['_'+id];},
  convertFileSrc(path){return path;},
  metadata:{currentWindow:{label:'main'},currentWebview:{label:'main'}}
};
window.__TASK18_SCENARIO__=scenario;
window.__TASK18_TRACE__=trace;
window.__TASK18_ERRORS__=[];
const originalConsoleError=console.error.bind(console);
console.error=(...values)=>{window.__TASK18_ERRORS__.push(values.map(String).join(' '));originalConsoleError(...values);};
addEventListener('error',event=>window.__TASK18_ERRORS__.push(String(event.error??event.message)));
addEventListener('unhandledrejection',event=>window.__TASK18_ERRORS__.push(String(event.reason)));
window.__TASK18_SET_PRIMING__=(value)=>{priming=Boolean(value);};
window.__TASK18_RESET_TRACE__=()=>{trace.commands.length=0;trace.preflights.length=0;trace.relink=null;};
})();</script>`;
}

const mime = new Map([[".html","text/html; charset=utf-8"],[".js","text/javascript; charset=utf-8"],[".css","text/css; charset=utf-8"],[".svg","image/svg+xml"],[".png","image/png"]]);
const server = createServer(async (request, response) => {
  try {
    const url = new URL(request.url, `http://127.0.0.1:${port}`);
    const requested = normalize(decodeURIComponent(url.pathname)).replace(/^[/\\]+/, "") || "index.html";
    let path = resolve(dist, requested);
    if (path !== dist && !path.startsWith(dist + sep)) throw new Error("path escaped dist");
    try { if (!(await stat(path)).isFile()) path = join(dist, "index.html"); } catch { path = join(dist, "index.html"); }
    let body = await readFile(path);
    if (extname(path) === ".html") body = Buffer.from(body.toString("utf8").replace("<head>", `<head>${bridgeSource()}`));
    response.writeHead(200, { "content-type": mime.get(extname(path)) ?? "application/octet-stream", "cache-control": "no-store" });
    response.end(body);
  } catch (error) { response.writeHead(500); response.end(String(error)); }
});
server.listen(port, "127.0.0.1", () => process.stdout.write(JSON.stringify({ ready: true, port, scenario }) + "\n"));
for (const signal of ["SIGINT","SIGTERM"]) process.on(signal, () => server.close(() => process.exit(0)));
