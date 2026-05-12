const $ = (id) => document.getElementById(id);
const fmtTime = (d = new Date()) =>
  `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}`;

let pc;
let dc;
let localStream;
let remoteStream;
let audioContext;
let analyser;
let meterTimer;
let pollTimer;
let activeChannelId;
let observationCount = 0;
let assistantDraft = "";
let turnLog = [];
let toolLog = [];
let remoteAudioSuppressed = false;
let interruptedResponseIds = new Set();

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

async function fetchJson(url, options = {}) {
  const response = await fetch(url, {
    ...options,
    headers: {
      "content-type": "application/json",
      ...(options.headers ?? {}),
    },
  });
  const body = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(body.error || `${response.status} ${response.statusText}`);
  }
  return body;
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

function waitForIceGatheringComplete(peerConnection) {
  if (peerConnection.iceGatheringState === "complete") {
    return Promise.resolve();
  }
  return new Promise((resolve) => {
    const onStateChange = () => {
      if (peerConnection.iceGatheringState === "complete") {
        peerConnection.removeEventListener("icegatheringstatechange", onStateChange);
        resolve();
      }
    };
    peerConnection.addEventListener("icegatheringstatechange", onStateChange);
  });
}

function setTransportField(id, value) {
  $(id).textContent = value || "-";
}

function resetUiForStart() {
  observationCount = 0;
  assistantDraft = "";
  turnLog = [];
  toolLog = [];
  remoteAudioSuppressed = false;
  interruptedResponseIds = new Set();
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
  remoteAudioSuppressed = true;
  const remote = $("remote");
  remote.pause();
  remote.srcObject = null;
}

function maybeResumeRemoteAudio(obs = {}) {
  const responseId = obs.response_id;
  if (responseId && interruptedResponseIds.has(responseId)) return;
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
      el.querySelector(".turn__role").textContent = turn.role;
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

function updateAssistantDraft(delta) {
  assistantDraft += delta || "";
  const last = turnLog[turnLog.length - 1];
  if (last?.role === "assistant" && last.draft) {
    last.text = assistantDraft;
  } else {
    turnLog.push({ role: "assistant", text: assistantDraft, time: fmtTime(), draft: true });
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
      updateAssistantDraft(obs.delta);
      break;
    case "assistant_transcript_final":
      commitTurn("assistant", obs.text || assistantDraft);
      assistantDraft = "";
      break;
    case "assistant_audio_chunk":
      maybeResumeRemoteAudio(obs);
      break;
    case "assistant_transcript_truncated":
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
      if (assistantDraft) {
        commitTurn("assistant", assistantDraft);
        assistantDraft = "";
      }
      logEvent("turn completed");
      break;
    case "turn_interrupted":
      suppressRemoteAudio(obs.response_id);
      logEvent(`turn interrupted${obs.response_id ? ` ${obs.response_id}` : ""}`);
      break;
    case "status_changed":
      logEvent(`status ${obs.status?.state || "changed"}`);
      break;
    case "command_rejected":
      logEvent(`command rejected: ${obs.message}`);
      break;
    case "error":
      setStatus("error", obs.message || "live error");
      logEvent(`error: ${obs.message || "unknown"}`);
      break;
    default:
      logEvent(obs.observation || "observation");
      break;
  }
}

async function pollState() {
  try {
    renderState(await fetchJson("/api/state"));
  } catch (error) {
    logEvent(`state poll failed: ${error.message}`);
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
  $("start").disabled = true;
  setStatus("pending", "creating session + microphone");
  resetUiForStart();
  try {
    const openPromise = fetchJson("/api/start", {
      method: "POST",
      body: JSON.stringify({
        model: $("model").value,
        workerModel: $("worker-model").value,
        turningMode: $("turning-mode").value,
      }),
    });

    pc = new RTCPeerConnection();
    pc.onconnectionstatechange = () => setTransportField("peer-state", pc.connectionState);
    pc.oniceconnectionstatechange = () => setTransportField("ice-state", pc.iceConnectionState);
    pc.onsignalingstatechange = () => setTransportField("signaling-state", pc.signalingState);
    pc.ontrack = (event) => {
      remoteStream = event.streams[0];
      if (!remoteAudioSuppressed) $("remote").srcObject = remoteStream;
      logEvent("remote audio track attached");
    };

    dc = pc.createDataChannel("meerkat-live");
    dc.onopen = () => {
      $("dc-state").textContent = "open";
      $("composer").dataset.live = "true";
      setStatus("live", "live");
      logEvent("data channel open");
    };
    dc.onclose = () => {
      $("dc-state").textContent = "closed";
      $("composer").dataset.live = "false";
      logEvent("data channel closed");
    };
    dc.onmessage = (message) => handleObservation(JSON.parse(message.data));

    localStream = await navigator.mediaDevices.getUserMedia({
      audio: {
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
      },
      video: false,
    });
    $("mic-state").textContent = "aec/noise/agc requested";
    $("audio-state").textContent = "microphone live";
    startMeter(localStream);
    pc.addTrack(localStream.getAudioTracks()[0], localStream);

    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    await waitForIceGatheringComplete(pc);
    const gatheredOffer = pc.localDescription;
    setStatus("pending", "signaling");
    const open = await openPromise;
    activeChannelId = open.channel_id;
    renderState(await fetchJson("/api/state"));
    const answer = await fetchJson("/api/webrtc/answer", {
      method: "POST",
      body: JSON.stringify({
        channel_id: open.channel_id,
        token: open.transport.token,
        offer_sdp: gatheredOffer.sdp,
      }),
    });
    $("sdp-state").textContent = `${gatheredOffer.sdp.length}/${answer.answer_sdp.length}`;
    await pc.setRemoteDescription({ type: "answer", sdp: answer.answer_sdp });
    setLiveControls(true);
    pollTimer = setInterval(pollState, 1200);
    logEvent(`live tools: ${open.tools.join(", ")}`);
  } catch (error) {
    setStatus("error", error.message);
    await cleanup(false);
  } finally {
    $("start").disabled = false;
  }
}

async function postLive(action, body = {}) {
  if (!activeChannelId) throw new Error("no active channel");
  return fetchJson(`/api/live/${encodeURIComponent(activeChannelId)}/${action}`, {
    method: "POST",
    body: JSON.stringify(body),
  });
}

async function cleanup(callServer = true) {
  if (pollTimer) clearInterval(pollTimer);
  pollTimer = undefined;
  stopMeter();
  dc?.close();
  pc?.close();
  localStream?.getTracks().forEach((track) => track.stop());
  remoteStream?.getTracks().forEach((track) => track.stop());
  if (callServer && activeChannelId) {
    await postLive("close").catch((error) => logEvent(`close failed: ${error.message}`));
  }
  pc = undefined;
  dc = undefined;
  localStream = undefined;
  remoteStream = undefined;
  activeChannelId = undefined;
  remoteAudioSuppressed = false;
  interruptedResponseIds = new Set();
  setLiveControls(false);
  $("composer").dataset.live = "false";
  $("dc-state").textContent = "closed";
  $("mic-state").textContent = "idle";
  $("audio-state").textContent = "microphone idle";
}

$("start").onclick = start;
$("stop").onclick = async () => {
  setStatus("pending", "stopping");
  await cleanup(true);
  setStatus("idle", "stopped");
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
    if (dc?.readyState === "open") {
      dc.send(JSON.stringify({ kind: "text", text }));
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
  dc?.close();
  pc?.close();
  localStream?.getTracks().forEach((track) => track.stop());
});

setStatus("idle", "idle");
setLiveControls(false);
pollState();
