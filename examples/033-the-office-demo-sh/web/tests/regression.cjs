// Execute the current application modules in Chrome, with controllable runtime
// boundaries, then bootstrap the built page with the real local WASM artifact.
const fs = require("node:fs");
const path = require("node:path");
const http = require("node:http");
const { spawn } = require("node:child_process");
const assert = require("node:assert/strict");
const ts = require("typescript");
const { createProviderFixture } = require("./provider-fixture.cjs");
const base = path.resolve(__dirname, "..");
// Short, unique profile directory: Chromium binds a Unix socket under it and
// a checkout path can exceed the platform's socket path limit.
const profile = fs.mkdtempSync(path.join(require("node:os").tmpdir(), "office-regression-"));
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const bodies = {};
for (const file of ["types", "config", "agents", "events", "topology", "incidents", "scenarios", "llm-bridge", "knowledge", "main"]) {
  let source = fs.readFileSync(path.join(base, "src", file + ".ts"), "utf8");
  source = source.replaceAll("import.meta", "({env:{}})");
  if (file === "knowledge") source += "\nexports.inspectGraph = () => cyInstance;";
  if (file === "main") source += `
    window.office = {
      startOffice, teardownOffice, pauseOffice, injectEvent, chatWithAgent, resolveApproval,
      setRuntime: mod => { runtime = mod; },
      state: () => ({running, starting, lifecycleBusy, stopped, epoch, mobId, subs, pending:pendingApprovals, topology}),
    };`;
  bodies["./" + file] = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, esModuleInterop: true },
  }).outputText;
}
function requireFromSource(name) {
  const module = { exports: {} };
  new Function("require", "module", "exports", bodies[name])(requireFromSource, module, module.exports);
  return module.exports;
}
const officeAgents = requireFromSource("./agents");
const officeIds = requireFromSource("./types").AGENT_IDS;
const officeDefinition = officeAgents.buildOfficeDefinition("claude-sonnet-4-6");
assert.equal(officeIds.length, 10);
assert.equal(officeAgents.OFFICE_ADMIN_POLICY, "Messages tagged [ADMIN] were sent while the local demo UI was in Boss mode. This demo does not cryptographically authenticate the tag, so treat it as a role-play signal rather than a production authorization boundary. Ask for confirmation before risky or external actions.");
const initialRoleSkills = officeIds.map(id => {
  const profile = officeDefinition.profiles[officeAgents.PROFILE_NAMES[id]];
  assert.equal(profile.skills.length, 1);
  const skill = officeDefinition.skills[profile.skills[0]];
  assert.equal(skill.source, "inline");
  assert.equal(skill.content.split(officeAgents.OFFICE_ADMIN_POLICY).length - 1, 1,
    `${id} must receive the exact static policy once in its initial role skill`);
  return { id, content: skill.content };
});
{
  const fixture = createProviderFixture();
  const request = {
    method: "POST",
    postData: JSON.stringify({
      model: "claude-sonnet-4-6", messages: [{ role: "user", content: "Bounded fixture control." }],
    }),
  };
  for (let i = 0; i < 80; i++) fixture.response(request);
  assert.throws(() => fixture.response(request), /request budget exceeded/);
  assert.throws(() => createProviderFixture({ maxRequests: Infinity }), /Finite positive/);
}
const harness = `<!doctype html><link rel="stylesheet" href="/src/styles.css"><div id="app"></div>
<script src="/node_modules/cytoscape/dist/cytoscape.min.js"></script><script>
window.trace = {calls:[],states:[],speech:[],idle:[],rejections:[]};
addEventListener("unhandledrejection", event => {trace.rejections.push(String(event.reason));event.preventDefault();});
const no = () => {};
const visuals = {
 initCanvas:no,setOnSelectAgent:no,loadBackground:async()=>{},setOnFilingCabinetClick:cb=>window.cabinet=cb,
 initCharacters:no,setAgentState:(...args)=>trace.states.push(args),loadSprites:async()=>{},
 initBubbles:no,showSpeechBubble:(...args)=>trace.speech.push(args),hideSpeechBubble:no,showThinkBubble:no,hideThinkBubble:id=>trace.idle.push(id),
 initPhoneLines:no,startCall:(...args)=>trace.calls.push(args),endCall:no,endCallsForAgent:no,triggerEnvelopeArrival:no,
};
const bodies=${JSON.stringify(bodies)}, cache={};
function require(name) {
 if(name === "./styles.css") return {};
 if(name.startsWith("./office/")) return visuals;
 if(name === "cytoscape") return window.cytoscape;
 if(cache[name]) return cache[name].exports;
 const module=cache[name]={exports:{}};
 new Function("require","module","exports",bodies[name])(require,module,module.exports);
 return module.exports;
}
require("./main");
window.mods = Object.fromEntries(["types","events","agents","topology","incidents","knowledge"].map(x=>[x,require("./"+x)]));
window.check = (ok,message) => {if(!ok)throw Error(message);};
window.delay = ms => new Promise(resolve=>setTimeout(resolve,ms));
window.until = async predicate => {for(let i=0;i<200;i++){if(predicate())return;await delay(10);}throw Error("Timed out waiting for state");};
window.click = id => document.getElementById(id).click();
window.visible = id => !!document.getElementById(id).getClientRects().length;
window.configure = () => {const key=document.getElementById("keyAnthropic");key.value="synthetic-not-a-key";key.dispatchEvent(new Event("change"));};
window.fixture = () => {
 const flags={}, calls=[], edges=new Set(), queues=new Map(), handles=new Set();
 let seq=0;
 const key=(a,b)=>[a,b].sort().join("|");
 const mod={
   default:async()=>{},
   init_runtime_from_config:()=>{calls.push(["init"]);if(flags.init)throw Error("init rejected");edges.clear();},
   destroy_runtime:()=>{calls.push(["destroy_runtime"]);handles.clear();edges.clear();},
   register_js_tool:no,
   mob_create:async()=>"the-office",
   mob_spawn:async(id,specs)=>{
     calls.push(["spawn"]);
     if(flags.holdSpawn)await new Promise(resolve=>flags.releaseSpawn=resolve);
     return JSON.stringify(JSON.parse(specs).map((spec,i)=>flags.spawn==="all"||(flags.spawn==="mixed"&&i===4)
       ? {status:"failed",result:{cause:"build_failed",message:"synthetic build failure"}}
       : {status:"spawned",result:{agent_identity:spec.agent_identity,member_ref:"member-"+i}}));
   },
   mob_member_peer_target:async(id,agent)=>{
     if(flags.peer===agent)throw Error("peer lookup failed");
     return JSON.stringify({external:{peer_id:"peer-"+agent,name:"misleading display name"}});
   },
   mob_wire:async(id,a,b)=>{calls.push(["wire",a,b]);if(flags.wire===true||flags.wire===key(a,b))throw Error("wire rejected");edges.add(key(a,b));},
   mob_unwire:async(id,a,b)=>{calls.push(["unwire",a,b]);if(flags.unwire===true||flags.unwire===key(a,b))throw Error("unwire rejected");edges.delete(key(a,b));},
   mob_append_system_context:async()=>{throw Error("Static startup policy must already be in initial skills");},
   mob_member_subscribe:async(id,agent)=>{if(flags.subscribe===agent)throw Error("subscription rejected");handles.add(agent);return agent;},
   close_subscription:handle=>{handles.delete(handle);},
   mob_lifecycle:async(id,action)=>{
     calls.push(["lifecycle",action]);if(flags.lifecycle)throw Error("stop rejected");
     if(flags.holdStop)await new Promise(resolve=>flags.releaseStop=resolve);
     return JSON.stringify({ok:true,mob_id:id,action});
   },
   mob_member_send:async(id,agent,request)=>{
     calls.push(["send",agent,JSON.parse(request)]);
     if(flags.holdSend)await new Promise(resolve=>flags.releaseSend=resolve);
     if(flags.send)throw Error("send rejected");
     return JSON.stringify({mob_id:id,agent_identity:agent,handling_mode:"queue"});
   },
   poll_subscription:handle=>{const events=queues.get(handle)||[];queues.set(handle,[]);return JSON.stringify(events);},
 };
 const push=(agent,payload,eventId)=>{
   const event={event_id:eventId||crypto.randomUUID(),source:{kind:"session",session_id:agent},seq:seq++,timestamp_ms:Date.now(),payload};
   queues.set(agent,[...(queues.get(agent)||[]),event]);return event;
 };
 return {mod,flags,calls,edges,handles,key,push,
   drain:()=>mods.events.drainAllEvents(mod,mods.types.AGENT_IDS.map(agentId=>({agentId,handle:agentId})))};
};
window.tool=(name,args,id="fc_0")=>({type:"tool_call_requested",id,name,args});
window.approval=(description="Same approval",risk="high")=>({short_summary:description,action_description:description,risk_level:risk,proposed_by:"Synthetic proposer"});
window.record=(id,summary="Initial summary")=>({id,title:id,type:"incident",summary,entities:[{name:"Alice",type:"person"},{name:"Office",type:"company"}],relationships:[{from:"Alice",to:"Office",type:"works_for"}]});
window.boot = async f => {office.setRuntime(f.mod);configure();click("startBigBtn");await until(()=>!office.state().starting);check(office.state().running,document.getElementById("statusLine").textContent);};
</script>`;

class CDP {
  constructor(ws) {
    this.ws = ws; this.id = 0; this.pending = new Map(); this.listeners = [];
    ws.addEventListener("message", event => {
      const message = JSON.parse(event.data);
      if (message.id) {
        const pending = this.pending.get(message.id); this.pending.delete(message.id);
        if (message.error) pending.reject(Error(JSON.stringify(message.error))); else pending.resolve(message.result);
      } else for (const listener of this.listeners) listener(message);
    });
  }
  static async connect(url) {
    const ws = new WebSocket(url);
    await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
    return new CDP(ws);
  }
  send(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++this.id; this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  async eval(expression) {
    let timer;
    const result = await Promise.race([
      this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }),
      new Promise((_, reject) => { timer = setTimeout(() => reject(Error("Browser evaluation exceeded 90 seconds")), 90000); }),
    ]).finally(() => clearTimeout(timer));
    if (result.exceptionDetails) throw Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  }
}

async function main() {
  // The --offline real-WASM checks load the built page and its synced
  // runtime. Without them the page 404s and only surfaces later as "Page
  // failed to start".
  if (process.argv.includes("--offline")) for (const built of ["dist/index.html", "dist/meerkat-pkg/meerkat_web_runtime.js", "dist/meerkat-pkg/meerkat_web_runtime_bg.wasm"]) {
    assert(fs.existsSync(path.join(base, built)),
      `${built} is missing; build the sdks/web runtime (cd sdks/web && npm run build), then run 'npm run build' here before 'npm run test:offline'`);
  }
  const server = http.createServer((req, res) => {
    const pathname = new URL(req.url, "http://localhost").pathname;
    if (pathname === "/fixture") { res.setHeader("Content-Type", "text/html"); res.end(harness); return; }
    const relative = "." + (pathname === "/" ? "/dist/index.html" : pathname);
    const file = [base, path.join(base, "dist")].map(root => path.resolve(root, relative))
      .find(candidate => fs.existsSync(candidate)) ?? path.resolve(base, relative);
    if (!file.startsWith(base + path.sep) || !fs.existsSync(file) || !fs.statSync(file).isFile()) {
      res.writeHead(404).end(); return;
    }
    const mime = { ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm", ".html": "text/html", ".png": "image/png" };
    res.setHeader("Content-Type", mime[path.extname(file)] || "application/octet-stream");
    fs.createReadStream(file).pipe(res);
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${server.address().port}`;
  // Playwright's bundled Chromium by default (`npx playwright install chromium`);
  // CHROME_BIN overrides it with any Chrome/Chromium binary. Like Playwright's
  // default launch, run without the Chromium sandbox unless CHROME_SANDBOX=1:
  // hosts that disable unprivileged user namespaces otherwise abort at start.
  const chromeBin = process.env.CHROME_BIN || require("playwright").chromium.executablePath();
  assert(fs.existsSync(chromeBin), `Chromium binary missing at ${chromeBin}; run 'npx playwright install chromium' or set CHROME_BIN`);
  const chrome = spawn(chromeBin, [
    "--headless=new", "--no-first-run", "--no-default-browser-check", "--disable-background-networking",
    "--disable-component-update", "--disable-sync", "--disable-extensions", "--remote-debugging-port=0",
    ...(process.env.CHROME_SANDBOX === "1" ? [] : ["--no-sandbox"]),
    "--user-data-dir=" + profile, "--disk-cache-dir=" + path.join(profile, "cache"), "about:blank",
  ], { env: { ...process.env, TMPDIR: profile }, stdio: ["ignore", "ignore", "pipe"] });
  let chromeStderr = "";
  chrome.stderr.on("data", chunk => { chromeStderr = (chromeStderr + chunk).slice(-4000); });
  // Registered before anything can fail so the teardown never waits for an
  // exit that already happened (a signal-killed Chrome has exitCode null).
  const chromeExited = new Promise(resolve => chrome.once("exit", resolve));
  const chromeAlive = () => chrome.exitCode === null && chrome.signalCode === null;
  let browser;
  const pages = [];
  let blocked = 0;
  try {
    const portFile = path.join(profile, "DevToolsActivePort");
    for (let i = 0; i < 200 && !fs.existsSync(portFile) && chromeAlive(); i++) await sleep(50);
    assert(fs.existsSync(portFile), `Chromium at ${chromeBin} did not expose DevTools${chromeAlive() ? "" : ` (exited: code ${chrome.exitCode}, signal ${chrome.signalCode})`}: ${chromeStderr.trim()}`);
    const [port, socket] = fs.readFileSync(portFile, "utf8").split("\n");
    browser = await CDP.connect(`ws://127.0.0.1:${port}${socket}`);
    async function page(route = "/fixture", provider = null) {
      const target = await (await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: "PUT" })).json();
      const p = await CDP.connect(target.webSocketDebuggerUrl);
      p.targetId = target.id;
      pages.push(p);
      p.console = [];
      p.networkErrors = [];
      p.fixtureErrors = [];
      p.fixturePending = new Set();
      const requestUrls = new Map();
      p.listeners.push(message => {
        if (message.method === "Runtime.consoleAPICalled") {
          p.console.push({ type: message.params.type, text: message.params.args.map(arg => arg.value ?? arg.description ?? arg.type).join(" ").slice(0, 2000) });
        }
        if (message.method === "Network.requestWillBeSent") requestUrls.set(message.params.requestId, message.params.request.url);
        if (message.method === "Network.loadingFailed") p.networkErrors.push({ url: requestUrls.get(message.params.requestId), error: message.params.errorText });
        if (message.method !== "Fetch.requestPaused") return;
        const { requestId, request } = message.params;
        if (new URL(request.url).origin === origin) void p.send("Fetch.continueRequest", { requestId });
        else if (provider?.accepts(request.url)) {
          try {
            provider.checkRequest?.(request);
            const pending = p.send("Fetch.fulfillRequest", { requestId, ...provider.response(request) });
            p.fixturePending.add(pending);
            void pending.then(() => p.fixturePending.delete(pending), error => {
              p.fixturePending.delete(pending);
              p.fixtureErrors.push(String(error));
            });
          } catch (error) {
            p.fixtureErrors.push(String(error));
            void p.send("Fetch.failRequest", { requestId, errorReason: "BlockedByClient" });
          }
        }
        else { blocked++; void p.send("Fetch.failRequest", { requestId, errorReason: "BlockedByClient" }); }
      });
      await p.send("Runtime.enable");
      await p.send("Network.enable");
      await p.send("Fetch.enable", { patterns: [{ urlPattern: "*" }] });
      await p.send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 900, deviceScaleFactor: 1, mobile: false });
      await p.send("Page.navigate", { url: origin + route });
      for (let i = 0; i < 200; i++) {
        if (await p.eval('!!document.getElementById("startBigBtn")')) return p;
        await sleep(25);
      }
      throw Error("Page failed to start");
    }
    async function test(name, body) {
      const p = await page();
      await p.eval(`(async()=>{${body};check(trace.rejections.length===0,"Unhandled rejection");})()`);
      console.log("PASS", name);
      // Close through the browser connection: a page session's own
      // Page.close reply can be lost when the target tears down its socket
      // first, which left the suite waiting forever under load.
      p.ws.close();
      await browser.send("Target.closeTarget", { targetId: p.targetId });
    }

    if (!process.argv.includes("--offline")) {
    await test("F01/F02 canonical peers and bounded envelope replay identity", `
      const f=fixture(), messages=[], approvals=[];
      mods.events.resetEventState();
      await mods.events.resolvePeerAgents(f.mod,"the-office");
      mods.events.setOnMessage((...x)=>messages.push(x));
      mods.events.setOnApprovalNeeded(x=>approvals.push(x));
      const args={peer_id:"peer-finance",display_name:"the-office/gate/gate",body:"hello"};
      f.push("triage",tool("send_message",args),"event-a");
      f.push("triage",tool("send_message",args),"event-a");
      f.push("it-dept",tool("send_message",args),"event-b");
      f.push("triage",tool("send_message",args),"event-c");
      f.push("triage",tool("send_message",{...args,peer_id:"unknown"}),"event-d");
      f.drain();
      check(trace.calls.length===3 && trace.calls.every(x=>x[1]==="finance"),"Canonical recipients / reused fc_0");
      check(messages.length===4 && messages.filter(x=>x[1]===null).length===1,"Unknown must not be guessed");
      check(messages.every(x=>x[3].includes("requested")),"Requests cannot claim delivery");
      mods.events.resetEventState();
      f.push("triage",tool("request_human_approval",approval()),"event-a");
      f.push("triage",tool("send_message",args),"new-event");
      f.drain();
      check(approvals.length===1 && trace.calls.length===3,"Restart resets replay cache and peer map");
      f.flags.peer="gate";let failed=false;try{await mods.events.resolvePeerAgents(f.mod,"the-office");}catch{failed=true;}
      check(failed,"Target-resolution failure must propagate");
      for(let i=0;i<4100;i++)f.push("triage",{type:"run_started"},"bounded-"+i);
      f.drain();f.push("triage",tool("request_human_approval",approval()),"event-a");f.drain();
      check(approvals.length===2,"Replay protection has a bounded recent-event horizon");
    `);

    await test("F03 current event terminal/extraction contract", `
      const f=fixture(), summaries=[];mods.events.setOnMessage((...x)=>summaries.push(x));
      f.push("triage",{type:"tool_execution_started"});f.drain();
      check(trace.states.at(-1)[1]==="on_call","Current tool execution start");
      for(const result of ["","Plain natural language"]){
        f.push("triage",{type:"run_completed",result,structured_output:{headline:"structured "+result,category:"analysis"}});
      }
      f.push("triage",{type:"run_completed",result:'{"headline":"legacy JSON","category":"response"}'});
      f.push("triage",{type:"run_completed",result:'{"headline":"premature"}',extraction_required:true});
      f.push("triage",{type:"extraction_succeeded",structured_output:{headline:"extracted"}});
      f.push("triage",{type:"extraction_succeeded",structured_output:{headline:"duplicate terminal"}});
      f.push("triage",{type:"run_completed",result:""});
      f.push("triage",{type:"run_failed",error_report:{message:"Anthropic authentication 401 rejected"}});
      const result=f.drain();
      check(summaries.length===4 && summaries.at(-1)[3]==="extracted","Exactly one summary per completion");
      check(trace.states.at(-1)[1]==="idle" && trace.idle.length>=7,"All terminals clear visuals");
      check(result.errors[0].includes("authentication 401"),"Current typed error report");
    `);

    await test("F05 visible cancel/retry and startup exclusion", `
      const f=fixture();office.setRuntime(f.mod);
      click("startBigBtn");await until(()=>!office.state().starting);
      check(visible("keyOverlay"),"Initial setup visible");click("keyDialogCancel");
      check(visible("startBtn"),"Visible start after cancel");click("gearBtn");configure();click("closeSettings");
      f.flags.init=true;click("startBtn");await until(()=>!office.state().starting);
      check(document.getElementById("statusBadge").textContent==="ERROR"&&visible("startBtn"),"Visible retry after failure");
      f.flags.init=false;f.flags.holdSpawn=true;click("startBtn");click("startBtn");
      await until(()=>!!f.flags.releaseSpawn);check(f.calls.filter(x=>x[0]==="spawn").length===1,"No overlapping startup");
      f.flags.releaseSpawn();await until(()=>!office.state().starting);
      check(office.state().running && office.state().subs.length===10 && f.edges.size===26,"Complete startup readiness");
    `);

    for (const [name, flags, expected] of [
      ["all spawn failures", { spawn: "all" }, "triage"],
      ["mixed spawn results", { spawn: "mixed" }, "finance"],
      ["wire failure", { wire: "it-dept|triage" }, "Wire"],
      ["Gate subscription failure", { subscribe: "gate" }, "Subscribe gate"],
      ["Archivist subscription failure", { subscribe: "archivist" }, "Subscribe archivist"],
      ["canonical peer failure", { peer: "finance" }, "peer lookup"],
    ]) {
      await test("F12 " + name, `
        const f=fixture();Object.assign(f.flags,${JSON.stringify(flags)});office.setRuntime(f.mod);configure();click("startBigBtn");
        await until(()=>!office.state().starting);
        check(document.getElementById("statusBadge").textContent==="ERROR","Must not become LIVE");
        check(document.getElementById("statusLine").textContent.includes(${JSON.stringify(expected)}),"Actionable stage/member");
        check(office.state().mobId===null && office.state().subs.length===0 && f.handles.size===0,"Partial runtime cleaned up");
        check(f.calls.some(x=>x[0]==="destroy_runtime")&&visible("startBtn"),"Cleanup and visible retry");
      `);
    }

    await test("F04 lifecycle admission, lag and failed stop", `
      const f=fixture();await boot(f);f.flags.holdStop=true;click("pauseBtn");
      await until(()=>!!f.flags.releaseStop);
      await office.injectEvent("server-alert");await office.chatWithAgent("triage","blocked");click("chatSend");
      check(f.calls.filter(x=>x[0]==="send").length===0,"Admission closed during stopping");
      check(document.getElementById("statusBadge").textContent==="STOPPING","Await stop before badge");
      f.flags.releaseStop();await until(()=>!office.state().lifecycleBusy);
      check(office.state().stopped&&!office.state().running,"Acknowledged stop");
      f.push("archivist",tool("upsert_record",record("while-stopping")));f.drain();
      check(mods.knowledge.getRecordCount()===1,"Host effects still drained");
      f.flags.holdStop=false;click("pauseBtn");await until(()=>!office.state().lifecycleBusy);
      check(office.state().running&&f.handles.size===10,"Resume renews subscriptions");
      f.push("triage",{type:"stream_truncated",reason:{kind:"stream_lagged",dropped:17}});
      await delay(350);check(document.getElementById("statusLine").textContent.includes("lost 17"),"Lag visibly reported");
      f.flags.lifecycle=true;click("pauseBtn");await until(()=>!office.state().lifecycleBusy);
      check(document.getElementById("statusBadge").textContent==="ERROR"&&!office.state().running&&!office.state().stopped,"No optimistic failed-stop badge");
    `);

    await test("F03 authentication recovery uses actual polling/dialog", `
      const f=fixture();await boot(f);
      f.push("gate",{type:"run_failed",error_report:{message:"Anthropic authentication 401 invalid API key"}});
      await until(()=>visible("keyOverlay"));
      check(f.calls.some(x=>x[0]==="lifecycle"&&x[1]==="stop"),"Recovery actually stops agents");
      check(document.getElementById("keyOverlayMessage").textContent.includes("invalid"),"Auth diagnostics reach dialog");
    `);

    await test("F06/F07/F08 safe independent approvals and retryable receipt delivery", `
      const f=fixture();await boot(f);
      const text='<b data-injected="yes">literal markup</b>';
      f.push("gate",tool("request_human_approval",{...approval(text),proposed_by:'<img data-injected="yes">'}),"approval-1");
      f.push("gate",tool("request_human_approval",approval(text)),"approval-2");
      f.push("gate",tool("request_human_approval",approval(text)),"approval-2");
      const prefix="a".repeat(65);
      f.push("gate",tool("request_human_approval",approval(prefix+"one")),"approval-3");
      f.push("gate",tool("request_human_approval",approval(prefix+"two")),"approval-4");
      f.push("gate",tool("request_human_approval",approval(text.toUpperCase())),"approval-5");f.drain();
      check(office.state().pending.length===5,"Only true replay deduplicated");
      check(!document.querySelector("[data-injected]"),"Compact text not markup");
      document.querySelector(".approval-item").click();
      check(!document.querySelector("[data-injected]")&&document.getElementById("approvalDetailBody").textContent.includes(text),"Expanded text not markup");
      check(!!document.querySelector(".risk-high"),"Fixed high-risk styling");
      f.flags.send=true;f.flags.holdSend=true;click("approveBtn");click("approveBtn");
      await until(()=>!!f.flags.releaseSend);
      check(f.calls.filter(x=>x[0]==="send").length===1,"Only one concurrent decision send");
      f.flags.releaseSend();await until(()=>office.state().pending[0].state==="failed");
      check(visible("approvalDetail")&&document.getElementById("approvalDetailBody").textContent.includes("Retry"),"Failure visible and retryable");
      check(document.getElementById("denyBtn").disabled,"Cannot change a pending decision");
      f.flags.send=false;f.flags.holdSend=false;click("approveBtn");
      await until(()=>office.state().pending.length===4);
      check(f.calls.filter(x=>x[0]==="send").length===2,"Retry has one accepted receipt");
      f.push("gate",tool("request_human_approval",approval(text)),"later-request");f.drain();
      check(office.state().pending.length===5,"Same wording after resolution is new");
      click("startBtn");await until(()=>!office.state().starting);
      check(office.state().pending.every(x=>x.state==="expired"),"Restart expires runtime-bound requests");
      const sends=f.calls.filter(x=>x[0]==="send").length;
      document.querySelector(".approve-mini").click();
      check(f.calls.filter(x=>x[0]==="send").length===sends,"Expired request cannot target new Gate");
    `);

    await test("F08 missing runtime and late receipt preserve request", `
      const f=fixture();await boot(f);
      f.push("gate",tool("request_human_approval",approval()));f.drain();
      office.setRuntime(null);document.querySelector(".approve-mini").click();
      await until(()=>office.state().pending[0].state==="failed");
      check(office.state().pending[0].error.includes("Runtime unavailable"),"Missing runtime not discarded");
      office.setRuntime(f.mod);f.flags.holdSend=true;document.querySelector(".approve-mini").click();
      await until(()=>!!f.flags.releaseSend);await office.teardownOffice();f.flags.releaseSend();await delay(20);
      check(office.state().pending[0].state==="expired","Late old-runtime receipt cannot clear expired request");
    `);

    await test("F06 fixed low/medium/high risk presentation and deny button", `
      const f=fixture();await boot(f);
      for(const risk of ["low","medium","high"]){
        f.push("gate",tool("request_human_approval",approval("Risk "+risk,risk)));f.drain();
        document.querySelector(".approval-item").click();
        check(!!document.querySelector(".risk-"+risk),"Expected fixed risk class "+risk);
        click("denyBtn");await until(()=>office.state().pending.length===0);
      }
      check(f.calls.filter(x=>x[0]==="send").length===3,"Each deny has a delivery attempt");
      check(f.calls.filter(x=>x[0]==="send").every(x=>x[2].content.startsWith("HUMAN DECISION: DENIED")),"Deny decision preserved");
    `);

    await test("F09 serialized truthful topology and endpoint intersection", `
      const f=fixture();await boot(f);
      f.push("it-dept",tool("revoke_access",{target:"finance",reason:"test"}));
      f.push("it-dept",tool("restore_access",{target:"finance",reason:"test"}));f.drain();
      await office.state().topology.settled();
      check(f.edges.size===26 && !office.state().topology.blocked.has("finance"),"Same-drain restore not lost");
      const top=office.state().topology;
      f.flags.unwire=true;const failed=await top.change("revoke","finance");
      check(failed.state==="failed"&&f.edges.size===26,"Total failure does not claim revoked");
      f.flags.unwire=f.key("finance","gate");const partial=await top.change("revoke","finance");
      check(partial.state==="partial" && f.edges.has(f.key("finance","gate")),"Partial edge failure");
      f.flags.unwire=false;check((await top.change("revoke","finance")).state==="complete","Retry reconciles unknown edges");
      await top.change("revoke","gate");await top.change("restore","finance");
      check(!f.edges.has(f.key("finance","gate")) && f.edges.has(f.key("finance","triage")),"Both endpoints must permit edge");
      f.flags.wire=true;const failedRestore=await top.change("restore","gate");
      check(failedRestore.state==="failed","Wire failure not claimed restored");
      f.flags.wire=false;await top.change("restore","gate");check(f.edges.size===26,"Wire retry reconciles");
      await top.change("revoke","finance");click("startBtn");await until(()=>!office.state().starting);
      check(office.state().topology.blocked.size===0&&f.edges.size===26,"Restart resets topology projection");
    `);

    await test("F10 visible Records and actual Cytoscape refresh only while shown", `
      click("tabCases");mods.knowledge.upsertRecord(record("case-a"));
      check(document.getElementById("kbContent").textContent.includes("Initial summary"),"Visible insert");
      mods.knowledge.upsertRecord({...record("case-a","Updated summary"),entities:[{name:"Bob",type:"person"}]});
      check(document.getElementById("kbContent").textContent.includes("Updated summary")&&document.getElementById("kbFooter").textContent.includes("3 ENTITIES"),"Visible update/footer");
      click("tabGraph");check(mods.knowledge.inspectGraph().nodes().length===3,"Real graph nodes");
      mods.knowledge.upsertRecord({...record("case-a"),entities:[{name:"Team",type:"company"}],relationships:[{from:"Bob",to:"Team",type:"works_for"}]});
      check(mods.knowledge.inspectGraph().nodes().length===4&&mods.knowledge.inspectGraph().edges().length===2,"Real graph update");
      click("tabLog");check(!mods.knowledge.isKBVisible()&&mods.knowledge.inspectGraph()===null,"Hidden state distinct");
      const before=document.getElementById("kbContent").innerHTML;
      mods.knowledge.upsertRecord(record("case-b","Hidden update"));
      check(mods.knowledge.inspectGraph()===null&&document.getElementById("kbContent").innerHTML===before,"No inactive rebuild");
      cabinet();check(document.getElementById("kbContent").textContent.includes("Hidden update"),"Cabinet opens current Records");
    `);

    await test("F11/F13 chronological uncorrelated activity and actual controls", `
      const f=fixture();await boot(f);
      document.querySelectorAll(".scenario-btn")[0].click();document.querySelectorAll(".scenario-btn")[1].click();
      document.getElementById("chatInput").value="Synthetic chat";document.getElementById("chatInput").dispatchEvent(new KeyboardEvent("keydown",{key:"Enter",bubbles:true}));
      f.push("finance",{type:"run_completed",result:"",structured_output:{headline:"Delayed reply to earlier work"}});
      f.push("it-dept",tool("revoke_access",{target:"finance",reason:"Synthetic access"}));
      f.push("gate",tool("request_human_approval",approval("Synthetic decision")));f.drain();
      await office.state().topology.settled();document.querySelector(".approve-mini").click();await delay(20);
      const incidents=mods.incidents.getIncidents();
      check(incidents.length===3&&incidents.every(x=>x.messages.length===1),"Only initial input owns scenario/chat source");
      const text=document.getElementById("panelContent").textContent;
      check(text.includes("UNCORRELATED ACTIVITY")&&text.includes("Delayed reply")&&text.includes("topology revoke")&&text.includes("decision accepted"),"Responses/access/approvals remain visible uncorrelated");
      check(text.indexOf("Synthetic chat")<text.indexOf("Delayed reply"),"Chronological order");
      check(document.querySelectorAll(".scenario-btn").length===6&&["tabLog","tabCases","tabGraph","pauseBtn","startBtn"].every(visible),"Documented controls exist");
    `);
    console.log("All 17 deterministic regression groups passed (actual TS/DOM; controlled runtime boundaries).");
    return;
    }

    // No fake runtime exports in this pass: the built application loads and
    // initializes its actual WASM. Only the exact Anthropic messages endpoint
    // receives bounded synthetic responses; every other off-origin request fails.
    // One control call; each Office initial/terminal-kickoff delivery can
    // produce one main call and one extraction. Each directed wire carries
    // at most one Started OR Cancelled kickoff notice in this scenario.
    const maxRequests = 1 + 2 * (officeIds.length + 2 * officeAgents.WIRING_PAIRS.length);
    const provider = createProviderFixture({ maxRequests });
    const p = await page("/dist/index.html", provider);
    const lifecycle = await p.eval(`(async()=>{
      const wasm=await import("/meerkat-pkg/meerkat_web_runtime.js");
      await wasm.default();
      wasm.init_runtime_from_config(JSON.stringify({model:"claude-sonnet-4-6",anthropic_api_key:"synthetic-not-a-key"}));
      const id=String(await wasm.mob_create(JSON.stringify({id:"lifecycle-probe",profiles:{worker:{model:"claude-sonnet-4-6",runtime_mode:"autonomous_host",tools:{comms:true},external_addressable:true}},wiring:{},flows:{}})));
      const spawned=JSON.parse(await wasm.mob_spawn(id,JSON.stringify([{profile:"worker",agent_identity:"worker",runtime_mode:"autonomous_host"}])));
      const stop=JSON.parse(await wasm.mob_lifecycle(id,"stop"));
      const stoppedStatus=JSON.parse(await wasm.mob_status(id));
      let rejected=false;try{await wasm.mob_member_send(id,"worker",JSON.stringify({content:"Synthetic stopped-admission probe",handling_mode:"queue"}));}catch{rejected=true;}
      const resume=JSON.parse(await wasm.mob_lifecycle(id,"resume"));
      const resumedStatus=JSON.parse(await wasm.mob_status(id));
      await wasm.mob_lifecycle(id,"destroy");wasm.destroy_runtime();
      return {spawned:spawned[0].status,stop:stop.ok,stopped:stoppedStatus.status,rejected,resume:resume.ok,resumed:resumedStatus.status};
    })()`);
    assert.deepEqual(lifecycle, { spawned: "spawned", stop: true, stopped: "Stopped", rejected: true, resume: true, resumed: "Running" });
    console.log("PASS real-WASM isolated lifecycle/admission (not full office topology or subscriptions)", JSON.stringify(lifecycle));
    const controlRequests = provider.requests.length;
    assert(controlRequests <= 1, "isolated non-structured lifecycle probe has at most one call");
    const initialRoleRequests = new Map();
    const lastRequestSize = new Map();
    const awaitingExtraction = new Set();
    const mainCalls = new Map();
    const extractionCalls = new Map();
    const noticeDeliveries = new Map();
    const workload = [];
    let workloadPhase = "startup";
    provider.checkRequest = request => {
      if (request.method !== "POST") return;
      const body = JSON.parse(request.postData);
      const system = typeof body.system === "string"
        ? body.system : body.system.map(block => block.text).join("\n");
      assert(system.includes(officeAgents.OFFICE_ADMIN_POLICY),
        "real system prefix must retain the static admin policy");
      const roles = initialRoleSkills.filter(role => system.includes(role.content));
      assert.equal(roles.length, 1, "request must identify its exact loaded role skill");
      const role = roles[0].id;
      const last = body.messages.at(-1);
      const extractionPrompt = "Provide the final output as valid JSON matching the required schema. Output ONLY the JSON, no additional text or markdown formatting.";
      if (last.role === "user" && last.content === extractionPrompt) {
        assert(awaitingExtraction.delete(role), `${role} extraction needs a new main call`);
        const count = (extractionCalls.get(role) ?? 0) + 1;
        assert(count <= (mainCalls.get(role) ?? 0), `${role} has at most one extraction per main call`);
        extractionCalls.set(role, count);
        // Extraction adds a real chronological User row, not new peer work.
        lastRequestSize.set(role, body.messages.length);
        workload.push({ role, phase: workloadPhase, kind: "extraction" });
        return;
      }
      mainCalls.set(role, (mainCalls.get(role) ?? 0) + 1);
      awaitingExtraction.add(role);
      if (body.messages.length === 1) {
        initialRoleRequests.set(role, (initialRoleRequests.get(role) ?? 0) + 1);
        assert.equal(initialRoleRequests.get(role), 1, `${role} has only one initial turn`);
        workload.push({ role, phase: workloadPhase, kind: "initial" });
      } else {
        const newUsers = body.messages.slice(lastRequestSize.get(role)).filter(message => message.role === "user");
        assert(newUsers.length > 0, `${role} followup requires newly admitted work`);
        const inThisTurn = new Set();
        for (const message of newUsers) {
          assert.equal(typeof message.content, "string");
          const notices = [...message.content.matchAll(/Intent: (mob\.kickoff_(?:started|cancelled))\nParams: ([\s\S]+?)\nRequest ID: ([0-9a-f-]+)/g)];
          assert(notices.length > 0, `${role} has unexpected non-kickoff work: ${JSON.stringify(message.content)}`);
          for (const [, intent, params, requestId] of notices) {
            if (inThisTurn.has(requestId)) continue; // The rendered notice repeats its request identity.
            inThisTurn.add(requestId);
            const sender = JSON.parse(params).peer;
            assert(officeAgents.WIRING_PAIRS.some(([a, b]) =>
              (a === role && b === sender) || (b === role && a === sender)), "notice must follow an actual edge");
            const edge = `${sender}|${role}`;
            assert(!noticeDeliveries.has(edge), `${edge} must not repeat a terminal kickoff delivery`);
            noticeDeliveries.set(edge, { requestId, intent });
          }
        }
        workload.push({ role, phase: workloadPhase, kind: "kickoff", inputs: inThisTurn.size });
      }
      lastRequestSize.set(role, body.messages.length);
    };
    await p.eval(`document.getElementById("startBigBtn").click()`);
    for (let i = 0; i < 100; i++) {
      if (await p.eval(`!!document.getElementById("keyOverlay").getClientRects().length`)) break;
      await sleep(20);
    }
    await p.eval(`document.getElementById("keyDialogAnthropic").value="synthetic-not-a-key";document.getElementById("keyDialogSave").click()`);
    let badge;
    let wiringState;
    for (let i = 0; i < 1200; i++) {
      badge = await p.eval(`document.getElementById("statusBadge").textContent`);
      if (["LIVE", "ERROR", "CONFIG"].includes(badge)) break;
      if (i >= 20 && !wiringState && await p.eval(`document.getElementById("statusLine").textContent==="Wiring comms topology..."`)) {
        wiringState = await p.eval(`(async()=>{
          const wasm=await import("/meerkat-pkg/meerkat_web_runtime.js");
          const members=JSON.parse(await wasm.mob_list_members("the-office"));
          const status=JSON.parse(await wasm.mob_status("the-office"));
          const snapshots=[], subscriptions=[];
          // The observation lane is bounded; do not saturate it with ten
          // simultaneous member_status calls while diagnosing a wire.
          for(const member of members){
            const agent_identity=member.agent_identity;
            try{
              const snapshot=JSON.parse(await wasm.mob_member_status("the-office",agent_identity));
              snapshots.push({agent_identity,status:snapshot.status,output_preview:snapshot.output_preview,tokens_used:snapshot.tokens_used,kickoff:snapshot.kickoff?.phase,progress:snapshot.progress,error:snapshot.error});
              const handle=await wasm.mob_member_subscribe("the-office",agent_identity);
              wasm.close_subscription(handle);
              subscriptions.push({agent_identity,subscribed:true});
            }catch(error){snapshots.push({agent_identity,error:String(error)});}
          }
          return {members:members.map(m=>({agent_identity:m.agent_identity,status:m.status,kickoff:m.kickoff?.phase,wired_to:m.wired_to})),status,snapshots,subscriptions};
        })()`);
      }
      await sleep(50);
    }
    if (badge !== "LIVE") {
      const failed = await p.eval(`(async()=>{
        const wasm=await import("/meerkat-pkg/meerkat_web_runtime.js");
        let cleaned=false;try{await wasm.mob_list_members("the-office");}catch{cleaned=true;}
        return {badge:document.getElementById("statusBadge").textContent,status:document.getElementById("statusLine").textContent,
          retryVisible:!!document.getElementById("startBtn").getClientRects().length,retryEnabled:!document.getElementById("startBtn").disabled,cleaned};
      })()`);
      assert(failed.cleaned && failed.retryVisible && failed.retryEnabled, "Real failed bootstrap must clean up and permit retry");
      console.log("PASS real-WASM failed-bootstrap cleanup/retry; healthy bootstrap BLOCKED", JSON.stringify(failed));
      const logs = p.console.filter(entry => /WARN|ERROR|failed|runtime boundary applied/.test(entry.text))
        .map(entry => ({ ...entry, text: entry.text.split("; color:")[0] }));
      console.log("OFFLINE DIAGNOSTICS", JSON.stringify({ requests: provider.requests, fixtureErrors: p.fixtureErrors, networkErrors: p.networkErrors, console: logs, wiringState }));
    }
    assert.equal(badge, "LIVE", await p.eval(`document.getElementById("statusLine").textContent`));
    assert.deepEqual([...initialRoleRequests.keys()].sort(), [...officeIds].sort(),
      "all ten real startup roles must exercise the provider boundary");
    for (const [id, count] of initialRoleRequests) {
      assert.equal(count, 1, `${id} must have exactly one initial provider request`);
    }
    console.log("PASS F-W4-01 static policy in all ten real initial system prefixes");
    const real = await p.eval(`(async()=>{
      const wasm=await import("/meerkat-pkg/meerkat_web_runtime.js");
      const members=JSON.parse(await wasm.mob_list_members("the-office"));
      const status=JSON.parse(await wasm.mob_status("the-office"));
      const targets=await Promise.all(members.map(m=>wasm.mob_member_peer_target("the-office",m.agent_identity)));
      const subscriptions=[];
      for(const member of members){
        const handle=await wasm.mob_member_subscribe("the-office",member.agent_identity);
        wasm.close_subscription(handle);subscriptions.push(member.agent_identity);
      }
      return {version:wasm.runtime_version(),members:members.length,status,peers:new Set(targets.map(t=>JSON.parse(t).external.peer_id)).size,
        topology:members.map(m=>({agent:m.agent_identity,peers:m.wired_to})),subscriptions};
    })()`);
    assert.equal(real.members, 10); assert.equal(real.peers, 10); assert.equal(real.status.status, "Running");
    const expected = requireFromSource("./agents").WIRING_PAIRS;
    assert.equal(real.topology.reduce((count, member) => count + member.peers.length, 0), expected.length * 2);
    for (const [a, b] of expected) {
      assert(real.topology.find(member => member.agent === a).peers.includes(b), `${a} must be wired to ${b}`);
      assert(real.topology.find(member => member.agent === b).peers.includes(a), `${b} must be wired to ${a}`);
    }
    assert.equal(real.subscriptions.length, 10);
    assert.equal(p.fixtureErrors.length, 0, JSON.stringify(p.fixtureErrors));
    workloadPhase = "stop";
    await p.eval(`document.getElementById("pauseBtn").click()`);
    for (let i = 0; i < 600; i++) {
      badge = await p.eval(`document.getElementById("statusBadge").textContent`);
      if (["STOPPED", "ERROR"].includes(badge)) break;
      await sleep(50);
    }
    assert.equal(badge, "STOPPED", await p.eval(`document.getElementById("statusLine").textContent`));
    const stopped = await p.eval(`(async()=>{
      const wasm=await import("/meerkat-pkg/meerkat_web_runtime.js");
      const status=JSON.parse(await wasm.mob_status("the-office"));
      let rejected=false;try{await wasm.mob_member_send("the-office","triage",JSON.stringify({content:"Synthetic stopped-admission probe",handling_mode:"queue"}));}catch{rejected=true;}
      return {status:status.status,rejected,disabled:document.getElementById("chatSend").disabled};
    })()`);
    assert.equal(stopped.status, "Stopped"); assert(stopped.rejected && stopped.disabled);
    workloadPhase = "resume";
    await p.eval(`document.getElementById("pauseBtn").click()`);
    for (let i = 0; i < 1200; i++) {
      badge = await p.eval(`document.getElementById("statusBadge").textContent`);
      if (["LIVE", "ERROR"].includes(badge)) break;
      await sleep(50);
    }
    assert.equal(badge, "LIVE", await p.eval(`document.getElementById("statusLine").textContent`));
    const resumed = await p.eval(`(async()=>{
      const wasm=await import("/meerkat-pkg/meerkat_web_runtime.js");
      const status=JSON.parse(await wasm.mob_status("the-office"));
      const members=JSON.parse(await wasm.mob_list_members("the-office"));
      const subscriptions=[];
      for(const member of members){const handle=await wasm.mob_member_subscribe("the-office",member.agent_identity);wasm.close_subscription(handle);subscriptions.push(member.agent_identity);}
      await wasm.mob_lifecycle("the-office","destroy");wasm.destroy_runtime();
      let destroyed=false;try{await wasm.mob_list_members("the-office");}catch{destroyed=true;}
      return {status:status.status,subscriptions:subscriptions.length,destroyed};
    })()`);
    assert.deepEqual(resumed, { status: "Running", subscriptions: 10, destroyed: true });
    assert.equal(p.fixturePending.size, 0, "destroy must leave no pending fixture responses");
    assert.equal(p.fixtureErrors.length, 0, JSON.stringify(p.fixtureErrors));
    assert.equal(provider.requests.length, controlRequests + workload.length);
    assert(noticeDeliveries.size <= 2 * officeAgents.WIRING_PAIRS.length);
    assert(provider.requests.length <= maxRequests);
    console.log("PASS bounded provider workload", JSON.stringify({
      limit: maxRequests, control: controlRequests,
      initial: [...initialRoleRequests.values()].reduce((a, b) => a + b, 0),
      kickoffInputs: noticeDeliveries.size,
      main: [...mainCalls.values()].reduce((a, b) => a + b, 0),
      extraction: [...extractionCalls.values()].reduce((a, b) => a + b, 0),
      byRoleAndPhase: workload.reduce((counts, row) => {
        const key = `${row.role}/${row.phase}/${row.kind}`;
        counts[key] = (counts[key] ?? 0) + 1;
        return counts;
      }, {}),
    }));
    console.log("PASS real-WASM offline bootstrap, stop/admission, resume", JSON.stringify(real));
    assert(provider.requests.length > 0, "Real provider boundary must be exercised");
    console.log(`All regression checks passed; ${provider.requests.length} synthetic Anthropic requests fulfilled; ${blocked} other off-origin requests blocked. No live-provider scenario executed.`);
  } finally {
    for (const p of pages) p.ws.close();
    browser?.ws.close();
    if (chromeAlive()) chrome.kill("SIGTERM");
    await chromeExited;
    await new Promise(resolve => server.close(resolve));
    // Chromium helper processes can still be flushing the profile right after
    // the browser process exits; retry, and never fail the run on cleanup.
    for (const deadline = Date.now() + 15_000; ; await sleep(200)) {
      try { fs.rmSync(profile, { recursive: true, force: true }); break; }
      catch (error) {
        if (Date.now() > deadline) { console.warn(`leaving browser profile ${profile}: ${error.message}`); break; }
      }
    }
  }
}
main().catch(error => { console.error(error); process.exitCode = 1; });
