(function(){const t=document.createElement("link").relList;if(t&&t.supports&&t.supports("modulepreload"))return;for(const r of document.querySelectorAll('link[rel="modulepreload"]'))a(r);new MutationObserver(r=>{for(const i of r)if(i.type==="childList")for(const s of i.addedNodes)s.tagName==="LINK"&&s.rel==="modulepreload"&&a(s)}).observe(document,{childList:!0,subtree:!0});function n(r){const i={};return r.integrity&&(i.integrity=r.integrity),r.referrerPolicy&&(i.referrerPolicy=r.referrerPolicy),r.crossOrigin==="use-credentials"?i.credentials="include":r.crossOrigin==="anonymous"?i.credentials="omit":i.credentials="same-origin",i}function a(r){if(r.ep)return;r.ep=!0;const i=n(r);fetch(r.href,i)}})();const M=document.querySelector("#app");if(!M)throw new Error("#app element missing");M.innerHTML=`
  <div class="shell">
    <header class="hero">
      <div class="hero-text">
        <p class="kicker">Meerkat WASM Arena</p>
        <h1>Mini Diplomacy Territory Control</h1>
        <p class="subtitle">Two autonomous mobs negotiate, commit hidden orders, and clash in a deterministic board resolver.</p>
      </div>
      <div class="hero-crests">
        <img src="/assets/crest-north.svg" alt="North crest" />
        <img src="/assets/crest-south.svg" alt="South crest" />
      </div>
    </header>

    <section class="controls card">
      <div class="control-grid">
        <label>North model
          <select id="northModel">
            <option value="claude-opus-4-6">claude-opus-4-6</option>
            <option value="gpt-5.2">gpt-5.2</option>
            <option value="gemini-3.1-pro-preview">gemini-3.1-pro-preview</option>
          </select>
        </label>
        <label>South model
          <select id="southModel">
            <option value="gpt-5.2">gpt-5.2</option>
            <option value="claude-opus-4-6">claude-opus-4-6</option>
            <option value="gemini-3.1-pro-preview">gemini-3.1-pro-preview</option>
          </select>
        </label>
        <label>Auth mode
          <select id="authMode">
            <option value="browser_byok">browser_byok</option>
            <option value="proxy">proxy</option>
          </select>
        </label>
        <label>Tick speed
          <select id="speedMs">
            <option value="1700">cinematic</option>
            <option value="1000" selected>normal</option>
            <option value="350">fast</option>
          </select>
        </label>
      </div>
      <div class="button-row">
        <button id="startBtn">Start Match</button>
        <button id="pauseBtn">Pause</button>
        <button id="stepBtn">Step</button>
        <button id="backBtn">Back</button>
        <button id="nextBtn">Forward</button>
        <button id="exportBtn">Export Replay</button>
        <label class="import-label">Import Replay
          <input id="importInput" type="file" accept="application/json" />
        </label>
      </div>
      <p id="statusLine" class="status">Idle</p>
    </section>

    <section class="layout">
      <article class="board card">
        <div class="board-header">
          <h2>Board + Score</h2>
          <span id="winnerBadge" class="badge">Winner: pending</span>
        </div>
        <div id="scoreStrip" class="score-strip"></div>
        <div id="regionGrid" class="region-grid"></div>
      </article>

      <article class="network card">
        <h2>Mob Network</h2>
        <div id="networkGraph"></div>
      </article>

      <article class="chatter card">
        <h2>Chatter Timeline</h2>
        <div id="chatterFeed" class="chatter-feed"></div>
      </article>
    </section>
  </div>
`;const x=document.querySelector("#northModel"),k=document.querySelector("#southModel"),_=document.querySelector("#authMode"),L=document.querySelector("#speedMs"),B=document.querySelector("#startBtn"),g=document.querySelector("#pauseBtn"),E=document.querySelector("#stepBtn"),N=document.querySelector("#backBtn"),O=document.querySelector("#nextBtn"),q=document.querySelector("#exportBtn"),R=document.querySelector("#importInput"),J=document.querySelector("#statusLine"),H=document.querySelector("#winnerBadge"),T=document.querySelector("#scoreStrip"),w=document.querySelector("#regionGrid"),I=document.querySelector("#networkGraph"),j=document.querySelector("#chatterFeed");if(!x||!k||!_||!L||!B||!g||!E||!N||!O||!q||!R||!J||!H||!T||!w||!I||!j)throw new Error("UI controls not found");let u=null,e=null,h=-1;function l(o){J.textContent=o}function F(){return{turn:1,max_turns:14,north_score:0,south_score:0,ruleset_version:"mini-diplomacy-v1",regions:[{id:"north-capital",controller:"north",defense:58,value:3},{id:"north-harbor",controller:"north",defense:48,value:2},{id:"north-ridge",controller:"north",defense:42,value:2},{id:"glass-frontier",controller:"south",defense:46,value:4},{id:"ember-crossing",controller:"south",defense:44,value:3},{id:"southern-rail",controller:"south",defense:47,value:2},{id:"saffron-fields",controller:"south",defense:40,value:2},{id:"ash-basin",controller:"south",defense:45,value:3},{id:"obsidian-gate",controller:"north",defense:43,value:3},{id:"mercury-delta",controller:"south",defense:38,value:2},{id:"crimson-pass",controller:"north",defense:40,value:3},{id:"south-capital",controller:"south",defense:56,value:3}]}}function f(o){return JSON.parse(o)}function G(o){return o.winner??"pending"}function A(o,t,n){o.push({seq:Date.now(),kind:"RESOLVE",from:"resolver",to:"board",team:t,payload:n})}function v(o){const t=o.state;H.textContent=`Winner: ${G(t)}`,T.innerHTML=`
    <div class="score north">North: ${t.north_score}</div>
    <div class="score meta">Turn ${t.turn}/${t.max_turns}</div>
    <div class="score south">South: ${t.south_score}</div>
  `,w.innerHTML="";for(const s of t.regions){const c=document.createElement("div");c.className=`region ${s.controller}`,c.innerHTML=`
      <h3>${s.id}</h3>
      <p>control: ${s.controller}</p>
      <p>defense: ${s.defense}</p>
      <p>value: ${s.value}</p>
    `,w.append(c)}const n=o.chatter.slice(-120).reverse();j.innerHTML=n.map(s=>{const c=`${s.team}.${s.from}->${s.to??"unknown"}`;return`<div class="event ${s.team}">
        <span class="tag">${s.kind}</span>
        <span class="edge">${c}</span>
        <p>${s.payload}</p>
      </div>`}).join("");const a=[{id:"north.planner",x:90,y:70},{id:"north.operator",x:90,y:190},{id:"resolver.board",x:280,y:130},{id:"south.planner",x:470,y:70},{id:"south.operator",x:470,y:190}],r=[];for(const[s,c]of o.edges.entries()){const[m,d]=s.split("->"),p=a.find(S=>S.id===m),b=a.find(S=>S.id===d);if(!p||!b)continue;const C=Math.min(11,1+c/2);r.push(`<line x1="${p.x}" y1="${p.y}" x2="${b.x}" y2="${b.y}" stroke-width="${C}" class="edge-line" />`)}const i=a.map(s=>{const c=s.id.replace("."," ");return`<g>
        <circle cx="${s.x}" cy="${s.y}" r="28" class="node-dot" />
        <text x="${s.x}" y="${s.y+4}" text-anchor="middle">${c}</text>
      </g>`}).join("");I.innerHTML=`
    <svg viewBox="0 0 560 260" role="img" aria-label="live network graph">
      ${r.join(`
`)}
      ${i}
    </svg>
  `}async function D(){if(u)return u;const t=await import(new URL("./runtime.js",window.location.href).toString());return await t.default(),u=t,t}async function $(o){const t=await fetch(o,{cache:"no-store"});if(!t.ok)throw new Error(`failed to fetch ${o}: ${t.status}`);const n=await t.arrayBuffer();return new Uint8Array(n)}function P(o,t){for(const n of o){const a=`${n.team}.${n.from}`,r=n.to==="board"?"resolver.board":`${n.team}.${n.to??"unknown"}`,i=`${a}->${r}`;t.set(i,(t.get(i)??0)+1)}}async function y(){var o,t;if(!(!u||!e||!e.running)){if(e.state.winner){e.running=!1,l(`Match complete. Winner=${e.state.winner}`),v(e);return}try{const n=JSON.stringify({state:e.state,opponent_signal:((o=e.frames.at(-1))==null?void 0:o.south.order.diplomacy)??"opening"}),a=JSON.stringify({state:e.state,opponent_signal:((t=e.frames.at(-1))==null?void 0:t.north.order.diplomacy)??"opening"}),r=f(u.submit_turn_input(e.northHandle,n)),i=f(u.submit_turn_input(e.southHandle,a)),s=f(u.resolve_turn(JSON.stringify({state:e.state,north_order:r.order,south_order:i.order}))),c=f(u.poll_events(e.northHandle)),m=f(u.poll_events(e.southHandle)),d=[...c,...m];A(d,"north",s.summary),P(d,e.edges),e.chatter.push(...d),e.state=s.state;const p={turn:e.state.turn,state:JSON.parse(JSON.stringify(e.state)),north:r,south:i,summary:s.summary,events:d};e.frames.push(p),h=e.frames.length-1,v(e),l(`Turn ${e.state.turn-1} resolved. North=${e.state.north_score} South=${e.state.south_score}`),e.running&&!e.state.winner&&window.setTimeout(()=>{y()},e.speedMs)}catch(n){e.running=!1;const a=n instanceof Error?n.message:String(n);l(`Runtime error: ${a}`)}}}async function W(){try{l("Loading runtime and mobpacks...");const o=await D(),[t,n]=await Promise.all([$("/north.mobpack"),$("/south.mobpack")]),a=x.value,r=k.value,i=_.value,s=Number(L.value),c=o.init_mobpack(t,JSON.stringify({team:"north",model:a,seed:101,auth_mode:i})),m=o.init_mobpack(n,JSON.stringify({team:"south",model:r,seed:202,auth_mode:i})),d=F(),p=JSON.stringify(d);o.start_match(c,p),o.start_match(m,p),e={northHandle:c,southHandle:m,northModel:a,southModel:r,authMode:i,state:d,frames:[],chatter:[],edges:new Map,running:!0,speedMs:s},h=-1,v(e),l(`Match started (${a} vs ${r}; mode=${i})`),await y()}catch(o){const t=o instanceof Error?o.message:String(o);l(`Failed to start match: ${t}`)}}function U(o){if(!e||e.frames.length===0)return;h=Math.max(0,Math.min(e.frames.length-1,h+o));const t=e.frames[h];e.state=JSON.parse(JSON.stringify(t.state)),v(e),l(`Replay frame ${h+1}/${e.frames.length}: ${t.summary}`)}function K(){if(!e||e.frames.length===0){l("No replay data to export yet.");return}const o={version:"mini-diplomacy-replay-v1",created_at:new Date().toISOString(),config:{north_model:e.northModel,south_model:e.southModel,auth_mode:e.authMode},frames:e.frames},t=new Blob([JSON.stringify(o,null,2)],{type:"application/json"}),n=URL.createObjectURL(t),a=document.createElement("a");a.href=n,a.download="mini-diplomacy-replay.json",a.click(),URL.revokeObjectURL(n),l("Replay exported.")}async function V(o){const t=await o.text(),n=f(t);if(!n.frames.length)throw new Error("Replay file has no frames");const a=n.frames[n.frames.length-1];e={northHandle:0,southHandle:0,northModel:n.config.north_model,southModel:n.config.south_model,authMode:n.config.auth_mode,state:JSON.parse(JSON.stringify(a.state)),frames:n.frames,chatter:n.frames.flatMap(r=>r.events),edges:new Map,running:!1,speedMs:1e3},P(e.chatter,e.edges),h=n.frames.length-1,v(e),l(`Replay imported (${n.frames.length} frames).`)}B.addEventListener("click",()=>{W()});g.addEventListener("click",()=>{e&&(e.running=!e.running,e.running?(g.textContent="Pause",y()):g.textContent="Resume")});E.addEventListener("click",()=>{e&&(e.running=!1,g.textContent="Resume",y())});N.addEventListener("click",()=>{U(-1)});O.addEventListener("click",()=>{U(1)});q.addEventListener("click",()=>{K()});R.addEventListener("change",o=>{var n;const t=(n=o.target.files)==null?void 0:n[0];t&&V(t).catch(a=>{const r=a instanceof Error?a.message:String(a);l(`Import failed: ${r}`)})});l("Ready. Build runtime artifacts with ../examples.sh, then Start Match.");
