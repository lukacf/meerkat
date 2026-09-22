import { LiveAttempt, fetchJson } from "./live-session.js";
import { Transcript } from "./transcript.js";

const $ = (id) => document.getElementById(id);
const fmtTime = (d = new Date()) =>
  `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}`;

let attempt;
let remoteStream;
let audioContext;
let analyser;
let meterTimer;
let observationCount = 0;
const transcript = new Transcript();
let turnLog = transcript.turns;
let toolLog = [];
let remoteAudioSuppressed = false;
let interruptedResponseIds = new Set();
let activeAudioResponseId;

function setStatus(state, text) {
  $("status").dataset.state = state;
  $("status-text").textContent = String(text).toUpperCase();
}

function setLiveControls(enabled) {
  for (const id of [
    "interrupt",
    "refresh",
    "stop",
    "send-text",
    "commit-audio",
    "commit-text",
    "truncate",
    "text-input",
  ]) {
    $(id).disabled = !enabled;
  }
}

function logEvent(text) {
  const el = document.createElement("div");
  el.className = "event";
  el.textContent = `${fmtTime()} ${text}`;
  $("events").prepend(el);
  while ($("events").children.length > 80) {
    $("events").lastChild.remove();
  }
}

function setTransportField(id, value) {
  $(id).textContent = value || "-";
}

function resetUiForStart() {
  observationCount = 0;
  transcript.reset();
  turnLog = transcript.turns;
  toolLog = [];
  remoteAudioSuppressed = false;
  interruptedResponseIds = new Set();
  activeAudioResponseId = undefined;
  $("transport-count").textContent = "0 obs";
  $("tool-count").textContent = "0";
  $("tools").innerHTML = "";
  $("events").innerHTML = "";
  $("mobs").innerHTML = "";
  $("notes").innerHTML = "";
  $("text-output").innerHTML = "";
  $("mob-count").textContent = "0";
  $("note-count").textContent = "0";
  $("text-output-count").textContent = "0";
  $("turns").innerHTML = "";
  renderTurns();
  renderTextOutputs([]);
}

function suppressRemoteAudio(responseId) {
  if (responseId) interruptedResponseIds.add(responseId);
  if (responseId && activeAudioResponseId && responseId !== activeAudioResponseId) return;
  if (!responseId && activeAudioResponseId) interruptedResponseIds.add(activeAudioResponseId);
  remoteAudioSuppressed = true;
  const remote = $("remote");
  remote.pause();
  remote.srcObject = null;
}

function maybeResumeRemoteAudio(obs = {}) {
  const responseId = obs.response_id;
  if (responseId && interruptedResponseIds.has(responseId)) return;
  if (remoteAudioSuppressed && !responseId) return;
  activeAudioResponseId = responseId;
  if (!remoteAudioSuppressed || !remoteStream) return;
  remoteAudioSuppressed = false;
  const remote = $("remote");
  remote.srcObject = remoteStream;
  remote.play().catch(() => undefined);
}

function renderTurns() {
  const root = $("turns");
  if (turnLog.length === 0) {
    root.innerHTML = `
      <div class="empty" id="empty-turns">
        <span class="label-mono">Live channel starting</span>
        <div class="empty__title">Speak when the status turns live</div>
        <div class="empty__body">Ask the agent to create a mob, save notes, interrupt it, or query mob status.</div>
      </div>
    `;
  } else {
    root.innerHTML = "";
    for (const turn of turnLog) {
      const el = document.createElement("div");
      el.className = `turn turn--${turn.role}`;
      el.innerHTML = `
        <div>
          <div class="turn__role"></div>
          <div class="turn__time"></div>
        </div>
        <div class="turn__body${turn.draft ? " turn__body--draft" : ""}"></div>
      `;
      el.querySelector(".turn__role").textContent = turn.interrupted ? `${turn.role} (interrupted)` : turn.role;
      el.querySelector(".turn__time").textContent = turn.time;
      el.querySelector(".turn__body").textContent = turn.text;
      root.appendChild(el);
    }
  }
  $("turn-count").textContent = `${turnLog.filter((turn) => !turn.draft).length} turns`;
  root.scrollTop = root.scrollHeight;
}

function commitTurn(role, text) {
  const clean = String(text || "").trim();
  if (!clean) return;
  const last = turnLog[turnLog.length - 1];
  if (last?.role === role && last.draft) {
    last.text = clean;
    last.draft = false;
  } else {
    turnLog.push({ role, text: clean, time: fmtTime(), draft: false });
  }
  renderTurns();
}

function renderTools() {
  $("tool-count").textContent = `${toolLog.length}`;
  if (toolLog.length === 0) {
    $("tools").innerHTML = `
      <div class="empty small">
        <span class="label-mono">Waiting</span>
        <div class="empty__body">Tool calls will appear here as observations arrive.</div>
      </div>
    `;
    return;
  }
  $("tools").innerHTML = "";
  for (const tool of toolLog) {
    const el = document.createElement("div");
    el.className = "tool";
    el.innerHTML = `
      <div class="row-head"><span></span><span class="meta-mono"></span></div>
      <div class="row-body"></div>
    `;
    el.querySelector(".row-head span:first-child").textContent = tool.name;
    el.querySelector(".row-head span:last-child").textContent = tool.time;
    el.querySelector(".row-body").textContent = tool.args;
    $("tools").prepend(el);
  }
}

function renderTextOutputs(outputs = []) {
  $("text-output-count").textContent = `${outputs.length}`;
  if (outputs.length === 0) {
    $("text-output").innerHTML = `
      <div class="empty small">
        <span class="label-mono">Waiting</span>
        <div class="empty__body">Long file lists, tables, code, logs, and search summaries should appear here instead of being read aloud.</div>
      </div>
    `;
    return;
  }
  $("text-output").innerHTML = "";
  for (const output of [...outputs].reverse()) {
    const el = document.createElement("article");
    el.className = "text-output";
    el.innerHTML = `
      <div class="row-head"><span></span><span class="meta-mono"></span></div>
      <pre></pre>
    `;
    el.querySelector(".row-head span:first-child").textContent = output.title || "Text output";
    el.querySelector(".row-head span:last-child").textContent = output.createdAt
      ? new Date(output.createdAt).toLocaleTimeString()
      : fmtTime();
    el.querySelector("pre").textContent = output.text || "";
    $("text-output").appendChild(el);
  }
}

function renderState(state) {
  $("session-id").textContent = state.session_id ? state.session_id.slice(0, 16) : "-";
  $("channel-id").textContent = state.channel_id ? state.channel_id.slice(0, 16) : "-";
  renderTextOutputs(state.text_outputs || []);

  const mobs = state.mobs || [];
  $("mob-count").textContent = `${mobs.length}`;
  if (mobs.length === 0) {
    $("mobs").innerHTML = `
      <div class="empty small">
        <span class="label-mono">None yet</span>
        <div class="empty__body">Say "create a mob that..." to exercise Meerkat mob creation from live voice.</div>
      </div>
    `;
  } else {
    $("mobs").innerHTML = "";
    for (const mob of mobs) {
      const el = document.createElement("div");
      el.className = "mob";
      el.innerHTML = `
        <div class="row-head"><span></span><span class="meta-mono"></span></div>
        <div class="row-sub"></div>
        <div class="members"></div>
      `;
      el.querySelector(".row-head span:first-child").textContent = mob.mob_id;
      el.querySelector(".row-head span:last-child").textContent = `${mob.members?.length || 0} members`;
      el.querySelector(".row-sub").textContent = mob.brief || "";
      const members = el.querySelector(".members");
      for (const member of mob.members || []) {
        const row = document.createElement("div");
        row.className = "member";
        row.innerHTML = "<span></span><span></span>";
        row.children[0].textContent = member.id;
        row.children[1].textContent = member.isFinal ? "final" : member.status;
        row.title = member.outputPreview || member.error || "";
        members.appendChild(row);
      }
      $("mobs").appendChild(el);
    }
  }

  const notes = state.notes || [];
  $("note-count").textContent = `${notes.length}`;
  if (notes.length === 0) {
    $("notes").innerHTML = `
      <div class="empty small">
        <span class="label-mono">None yet</span>
        <div class="empty__body">Say "remember..." or "save a note..." to exercise callback tools.</div>
      </div>
    `;
  } else {
    $("notes").innerHTML = "";
    for (const note of notes) {
      const el = document.createElement("div");
      el.className = "note";
      el.innerHTML = `
        <div class="row-head"><span></span><span class="meta-mono"></span></div>
      `;
      el.querySelector(".row-head span:first-child").textContent = note.text;
      el.querySelector(".row-head span:last-child").textContent = new Date(note.createdAt).toLocaleTimeString();
      $("notes").prepend(el);
    }
  }
}

function handleObservation(obs) {
  observationCount += 1;
  $("transport-count").textContent = `${observationCount} obs`;

  switch (obs.observation) {
    case "ready":
      logEvent("adapter ready");
      break;
    case "user_transcript_final":
      commitTurn("user", obs.text);
      break;
    case "assistant_text_delta":
    case "assistant_transcript_delta":
      transcript.delta(obs, obs.observation === "assistant_text_delta" ? "written" : "spoken", fmtTime());
      renderTurns();
      break;
    case "assistant_transcript_final":
      transcript.final(obs, fmtTime());
      renderTurns();
      break;
    case "assistant_audio_chunk":
      maybeResumeRemoteAudio(obs);
      break;
    case "assistant_transcript_truncated":
      transcript.interrupt(obs, fmtTime(), true);
      renderTurns();
      suppressRemoteAudio(obs.response_id);
      logEvent("assistant transcript truncated");
      break;
    case "tool_call_requested":
      toolLog.push({
        name: obs.tool_name || "tool",
        args: JSON.stringify(obs.arguments ?? {}),
        time: fmtTime(),
      });
      renderTools();
      logEvent(`tool requested: ${obs.tool_name}`);
      break;
    case "turn_completed":
      transcript.complete(obs);
      renderTurns();
      logEvent("turn completed");
      break;
    case "turn_interrupted":
      transcript.interrupt(obs, fmtTime());
      renderTurns();
      suppressRemoteAudio(obs.response_id);
      logEvent(`turn interrupted${obs.response_id ? ` ${obs.response_id}` : ""}`);
      break;
    case "status_changed":
      logEvent(`status ${obs.status?.status || "changed"}`);
      if (obs.status?.status === "closed" && attempt) void terminate(attempt, "idle", "channel closed");
      break;
    case "command_rejected":
      logEvent(`command rejected: ${obs.message}`);
      break;
    case "error":
      if (attempt) void terminate(attempt, "error", obs.message || "live error");
      logEvent(`error: ${obs.message || "unknown"}`);
      break;
    default:
      logEvent(obs.observation || "observation");
      break;
  }
}

async function pollState(owner = attempt) {
  try {
    const state = await fetchJson("/api/state", owner ? { signal: owner.signal } : {});
    if (owner === attempt && !owner?.disposed) renderState(state);
  } catch (error) {
    if (owner === attempt && !owner?.disposed) logEvent(`state poll failed: ${error.message}`);
  }
}

function startMeter(stream) {
  stopMeter();
  audioContext = new AudioContext();
  analyser = audioContext.createAnalyser();
  analyser.fftSize = 256;
  audioContext.createMediaStreamSource(stream).connect(analyser);
  const data = new Uint8Array(analyser.frequencyBinCount);
  const bars = [...$("meter").querySelectorAll("span")];
  meterTimer = setInterval(() => {
    analyser.getByteFrequencyData(data);
    const avg = data.reduce((sum, value) => sum + value, 0) / data.length;
    bars.forEach((bar, index) => {
      const scale = Math.max(5, Math.min(20, avg / 6 + index * 2));
      bar.style.height = `${scale}px`;
    });
  }, 80);
}

function stopMeter() {
  if (meterTimer) clearInterval(meterTimer);
  meterTimer = undefined;
  if (audioContext) audioContext.close().catch(() => undefined);
  audioContext = undefined;
  analyser = undefined;
}

async function start() {
  if (attempt) return;
  $("start").disabled = true;
  $("stop").disabled = false;
  setStatus("pending", "creating session + microphone");
  resetUiForStart();
  let owner;
  try {
    owner = new LiveAttempt((error) => logEvent(`close failed: ${error.message}`));
  } catch (error) {
    setStatus("error", error.message);
    setLiveControls(false);
    $("start").disabled = false;
    return;
  }
  attempt = owner;
  const { pc, dc } = owner;
  const current = () => attempt === owner && !owner.disposed;
  const terminal = (state, message) => {
    if (current()) void terminate(owner, state, message);
  };
  try {
    owner.open({
      model: $("model").value,
      workerModel: $("worker-model").value,
      turningMode: $("turning-mode").value,
    });
    pc.onconnectionstatechange = () => {
      if (!current()) return;
      setTransportField("peer-state", pc.connectionState);
      if (["failed", "closed"].includes(pc.connectionState)) terminal("error", `peer ${pc.connectionState}`);
    };
    pc.oniceconnectionstatechange = () => {
      if (!current()) return;
      setTransportField("ice-state", pc.iceConnectionState);
      if (["failed", "closed"].includes(pc.iceConnectionState)) terminal("error", `ICE ${pc.iceConnectionState}`);
    };
    pc.onsignalingstatechange = () => {
      if (current()) setTransportField("signaling-state", pc.signalingState);
    };
    pc.ontrack = (event) => {
      if (!current()) { event.track.stop(); return; }
      remoteStream = owner.ownStream(event.streams[0] || new MediaStream([event.track]));
      if (!remoteAudioSuppressed) $("remote").srcObject = remoteStream;
      logEvent("remote audio track attached");
    };
    dc.onopen = () => {
      if (!current()) return;
      $("dc-state").textContent = "open";
      logEvent("data channel open");
    };
    dc.onclose = () => terminal("idle", "data channel closed");
    dc.onmessage = (message) => {
      if (current()) handleObservation(JSON.parse(message.data));
    };
    const localStream = await owner.microphone();
    if (!current()) return;
    $("mic-state").textContent = "aec/noise/agc requested";
    $("audio-state").textContent = "microphone live";
    owner.cleanups.push(stopMeter);
    startMeter(localStream);
    pc.addTrack(localStream.getAudioTracks()[0], localStream);
    setStatus("pending", "signaling");
    const { open, offerSdp, answerSdp } = await owner.negotiate();
    if (!current()) return;
    $("sdp-state").textContent = `${offerSdp.length}/${answerSdp.length}`;
    await owner.connected();
    if (!current()) return;
    setStatus("live", "live");
    $("composer").dataset.live = "true";
    setLiveControls(true);
    const pollTimer = setInterval(() => void pollState(owner), 1200);
    owner.cleanups.push(() => clearInterval(pollTimer));
    void pollState(owner);
    logEvent(`live tools: ${open.tools.join(", ")}`);
  } catch (error) {
    if (current()) await terminate(owner, "error", error.message);
    else await owner.dispose();
  }
}

async function postLive(action, body = {}) {
  const owner = attempt;
  if (!owner?.openResult || owner.disposed) throw new Error("no active channel");
  return fetchJson(`/api/live/${encodeURIComponent(owner.openResult.channel_id)}/${action}`, {
    method: "POST",
    body: JSON.stringify(body),
    signal: owner.signal,
  });
}

async function terminate(owner, state, message) {
  if (attempt !== owner || owner.disposed) return;
  setStatus(state, message);
  setLiveControls(false);
  $("remote").pause();
  $("remote").srcObject = null;
  remoteStream = undefined;
  remoteAudioSuppressed = false;
  interruptedResponseIds = new Set();
  $("composer").dataset.live = "false";
  $("dc-state").textContent = "closed";
  $("mic-state").textContent = "idle";
  $("audio-state").textContent = "microphone idle";
  await owner.dispose();
  if (attempt === owner) {
    attempt = undefined;
    $("start").disabled = false;
  }
}

$("start").onclick = start;
$("stop").onclick = async () => {
  if (attempt) await terminate(attempt, "idle", "stopped");
};
$("interrupt").onclick = async () => {
  try {
    suppressRemoteAudio();
    await postLive("interrupt");
    logEvent("interrupt sent");
  } catch (error) {
    logEvent(`interrupt failed: ${error.message}`);
  }
};
$("refresh").onclick = async () => {
  try {
    await postLive("refresh");
    logEvent("refresh sent");
  } catch (error) {
    logEvent(`refresh failed: ${error.message}`);
  }
};
$("commit-audio").onclick = async () => {
  try {
    await postLive("commit", { response_modality: "audio" });
    logEvent("commit audio sent");
  } catch (error) {
    logEvent(`commit failed: ${error.message}`);
  }
};
$("commit-text").onclick = async () => {
  try {
    await postLive("commit", { response_modality: "text" });
    logEvent("commit text sent");
  } catch (error) {
    logEvent(`commit failed: ${error.message}`);
  }
};
$("truncate").onclick = async () => {
  try {
    await postLive("truncate", {
      item_id: $("truncate-item").value,
      content_index: 0,
      audio_played_ms: Number($("truncate-ms").value || 0),
    });
    logEvent("truncate sent");
  } catch (error) {
    logEvent(`truncate failed: ${error.message}`);
  }
};
$("text-form").onsubmit = async (event) => {
  event.preventDefault();
  const text = $("text-input").value.trim();
  if (!text) return;
  try {
    if (attempt?.dc.readyState === "open") {
      attempt.dc.send(JSON.stringify({ kind: "text", text }));
      logEvent("data channel text chunk sent");
    } else {
      await postLive("text", { text });
      logEvent("rpc text chunk sent");
    }
    $("text-input").value = "";
  } catch (error) {
    logEvent(`text send failed: ${error.message}`);
  }
};

window.addEventListener("beforeunload", () => {
  if (attempt) void terminate(attempt, "idle", "stopped");
});

setStatus("idle", "idle");
setLiveControls(false);
pollState();
