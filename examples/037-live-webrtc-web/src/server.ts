import { randomUUID } from "node:crypto";
import { createReadStream, existsSync } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { extname, join, normalize, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  isLiveWebrtcBootstrap,
  LiveChannel,
  MeerkatClient,
  type WireLiveResponseModality,
  type WireLiveTransportBootstrapWebrtc,
} from "@rkat/sdk";

const EXAMPLE_ROOT = resolve(fileURLToPath(new URL("..", import.meta.url)));
const PUBLIC_ROOT = join(EXAMPLE_ROOT, "public");
const DEFAULT_LIVE_MODEL = process.env.MEERKAT_LIVE_MODEL ?? "gpt-realtime-2";
const DEFAULT_WORKER_MODEL = process.env.MEERKAT_LIVE_WORKER_MODEL ?? "gpt-5.5";

interface ToolDefinition extends Record<string, unknown> {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
}

interface StartRequest {
  model?: string;
  workerModel?: string;
  turningMode?: "provider_managed" | "explicit_commit";
}

interface WebrtcAnswerRequest {
  channel_id: string;
  token: string;
  offer_sdp: string;
}

interface TextInputRequest {
  text?: string;
  commit?: boolean;
  responseModality?: "audio" | "text";
}

interface SavedNote {
  id: string;
  text: string;
  createdAt: string;
}

interface TextPaneOutput {
  id: string;
  title: string;
  text: string;
  createdAt: string;
}

interface DisplayMember {
  id: string;
  profile: string;
  status: string;
  outputPreview?: string;
  isFinal: boolean;
  error?: string;
}

interface DisplayMob {
  mob_id: string;
  brief: string;
  roles: string[];
  status: string;
  members: DisplayMember[];
}

interface ActiveLiveSession {
  sessionId: string;
  channel: LiveChannel;
  channelId: string;
  transport: WireLiveTransportBootstrapWebrtc;
  model: string;
  workerModel: string;
  notes: SavedNote[];
  textOutputs: TextPaneOutput[];
}

interface AppRuntime {
  client: MeerkatClient;
}

let runtimePromise: Promise<AppRuntime> | undefined;
let activeLive: ActiveLiveSession | undefined;

const CALLBACK_TOOL_DEFS: ToolDefinition[] = [
  {
    name: "voice_session_note",
    description: "Save a short note from the live voice smoke test.",
    input_schema: {
      type: "object",
      properties: {
        text: { type: "string", description: "The note to save." },
      },
      required: ["text"],
      additionalProperties: false,
    },
  },
  {
    name: "voice_session_text_pane",
    description:
      "Show longer text-only output in the browser smoke-test text pane instead of reading it aloud.",
    input_schema: {
      type: "object",
      properties: {
        title: {
          type: "string",
          description: "Short label for the text pane entry.",
        },
        text: {
          type: "string",
          description: "Full text to display in the browser text pane.",
        },
      },
      required: ["text"],
      additionalProperties: false,
    },
  },
];

const MOB_MCP_TOOL_NAMES = [
  "delegate",
  "mob_create",
  "mob_destroy",
  "mob_spawn_member",
  "mob_retire_member",
  "mob_check_member",
  "mob_list_members",
  "mob_list",
  "mob_wire",
  "mob_unwire",
];

const LIVE_CONTROLLER_BLOCKED_TOOLS = ["mob_check_member"];
const LIVE_CONTROLLER_VISIBLE_MOB_TOOLS = MOB_MCP_TOOL_NAMES.filter(
  (name) => !LIVE_CONTROLLER_BLOCKED_TOOLS.includes(name),
);

function smokeTestInstructions(controllerCommsName: string, workerModel: string): string {
  return `You are the voice controller for a manual Meerkat Live WebRTC smoke test.

Your job is to help the human verify that browser WebRTC, the Meerkat live
adapter, callback tools, and mob orchestration all work through one live
conversation.

This live controller session is comms-addressable as ${controllerCommsName}.
Delegate helpers should see that controller in peers() after delegate wiring
succeeds. The controller is an external peer for implicit delegation, not a
local mob member named "orchestrator". Do not claim that an implicit mob has a
wireable local orchestrator member unless mob_list_members actually shows one.

Use tools aggressively when asked:
- If the user says "remember", "note", or "save", call voice_session_note.
- If the user asks to create a mob, team, swarm, panel, review group, research
  group, or similar, use the real Meerkat mob-mcp tools: mob_create and then
  mob_spawn_member as needed. Do not pretend to create a mob in plain text.
- For quick helper/delegate smoke tests, prefer delegate with explicit tooling
  {"mode":"profile","source":{"type":"inline","model":"${workerModel}","tools":{"builtins":true,"shell":true,"comms":true}}}.
  Ask the helper to call peers() and send_message back to ${controllerCommsName}
  when it finishes. If the delegate result has wired=false, say that plainly.
  Do not poll helper status through mob_check_member; helper completion must be
  observed through normal comms or through ordinary user-visible output.
- If the user asks a created mob/member to do follow-up work, use
  mob_spawn_member, delegate, mob_wire, or the appropriate mob-mcp tool. Only
  use mob_wire for members that mob_list_members shows as local mob members.
- If the user asks what exists, use mob_list or mob_list_members for roster
  inspection only. Do not use roster/status tools as a substitute for comms.
- For long, dense, or visually scannable outputs such as file listings,
  directory trees, tables, code, logs, search result lists, or anything longer
  than a few short sentences, do not read the full content aloud. Say a brief
  preamble like "Showing in text pane." and call voice_session_text_pane with
  the full text.

Keep spoken replies short. Mention concrete mob ids and member identities after
tool calls so the browser cockpit can be compared with what you said.`;
}

function registerCallbackTools(client: MeerkatClient): void {
  const noteTool = CALLBACK_TOOL_DEFS.find((tool) => tool.name === "voice_session_note");
  if (noteTool) {
    client.registerTool(noteTool.name, noteTool.description, noteTool.input_schema, async (args) => {
      const text = String(args.text ?? "").trim();
      if (!text) return "No note text was provided.";
      const live = activeLive;
      if (!live) return "No active live smoke-test session.";
      live.notes.push({
        id: randomUUID(),
        text,
        createdAt: new Date().toISOString(),
      });
      return `Saved note #${live.notes.length}.`;
    });
  }

  const textPaneTool = CALLBACK_TOOL_DEFS.find((tool) => tool.name === "voice_session_text_pane");
  if (textPaneTool) {
    client.registerTool(
      textPaneTool.name,
      textPaneTool.description,
      textPaneTool.input_schema,
      async (args) => {
        const text = String(args.text ?? "").trim();
        if (!text) return "No text pane content was provided.";
        const live = activeLive;
        if (!live) return "No active live smoke-test session.";
        const title = String(args.title ?? "").trim() || "Text output";
        live.textOutputs.push({
          id: randomUUID(),
          title,
          text,
          createdAt: new Date().toISOString(),
        });
        return `Showing text pane entry #${live.textOutputs.length}.`;
      },
    );
  }
}

async function ensureRuntime(): Promise<AppRuntime> {
  if (runtimePromise) return runtimePromise;
  runtimePromise = (async () => {
    const rkatPath = process.env.RKAT_RPC ?? process.env.MEERKAT_BIN_PATH ?? "rkat-rpc";
    const client = new MeerkatClient(rkatPath);
    registerCallbackTools(client);
    await client.connect({
      contextRoot: EXAMPLE_ROOT,
      realmId: process.env.MEERKAT_REALM,
      isolated: process.env.MEERKAT_REALM == null,
      liveWebrtc: true,
      liveToolTimeoutMs: Number(process.env.MEERKAT_LIVE_TOOL_TIMEOUT_MS ?? 180_000),
    });
    client.requireCapability("mob");
    return { client };
  })().catch((error) => {
    runtimePromise = undefined;
    throw error;
  });
  return runtimePromise;
}

function asWireModality(value: unknown): WireLiveResponseModality | undefined {
  if (value === "audio" || value === "text") {
    return { modality: value };
  }
  return undefined;
}

async function pollMobs(): Promise<DisplayMob[]> {
  if (!activeLive) return [];
  const { client } = await ensureRuntime();
  const summaries = await client.listMobs().catch(() => []);
  const result: DisplayMob[] = [];
  for (const summary of summaries) {
    const mobId = summary.mobId;
    const mob = client.mob(mobId);
    const members: DisplayMember[] = [];
    let status = summary.status;
    try {
      const listed = await mob.listMembers();
      for (const member of listed) {
        let snapshot:
          | {
              status: string;
              outputPreview?: string;
              isFinal: boolean;
              error?: string;
            }
          | undefined;
        try {
          snapshot = await mob.memberStatus(member.agentIdentity);
        } catch {
          snapshot = undefined;
        }
        members.push({
          id: member.agentIdentity,
          profile: member.profile,
          status: snapshot?.status ?? member.status ?? "known",
          outputPreview: snapshot?.outputPreview,
          isFinal: snapshot?.isFinal ?? Boolean(member.isFinal),
          error: snapshot?.error ?? member.error,
        });
      }
    } catch (error) {
      members.push({
        id: "member-list",
        profile: "diagnostic",
        status: "error",
        isFinal: true,
        error: error instanceof Error ? error.message : String(error),
      });
    }
    result.push({
      mob_id: mobId,
      brief: "",
      roles: members.map((member) => member.profile),
      status,
      members,
    });
  }
  return result;
}

async function startLive(body: StartRequest): Promise<unknown> {
  const { client } = await ensureRuntime();
  if (activeLive) {
    await activeLive.channel.close().catch(() => undefined);
    activeLive = undefined;
  }

  const model = body.model?.trim() || DEFAULT_LIVE_MODEL;
  const workerModel = body.workerModel?.trim() || DEFAULT_WORKER_MODEL;
  const controllerCommsName = `live-smoke-controller-${randomUUID()}`;
  const session = await client.createDeferredSession(
    "Start a browser WebRTC live smoke-test control session.",
    {
      model,
      systemPrompt: smokeTestInstructions(controllerCommsName, workerModel),
      enableBuiltins: true,
      enableShell: true,
      enableSchedule: true,
      enableMob: true,
      enableWebSearch: true,
      toolFilter: { Deny: LIVE_CONTROLLER_BLOCKED_TOOLS },
      keepAlive: true,
      commsName: controllerCommsName,
      peerMeta: {
        description: "Browser WebRTC smoke-test controller session.",
        labels: {
          example: "037-live-webrtc-web",
          smoke_role: "live-controller",
        },
      },
      externalTools: CALLBACK_TOOL_DEFS,
      labels: {
        example: "037-live-webrtc-web",
        surface: "json-rpc-live-webrtc",
      },
      additionalInstructions: [
        "This is a manual smoke test. Prefer real tool calls over explanations.",
        "Shell is enabled for smoke-test filesystem commands such as listing the current directory.",
        "Schedule tools are enabled for smoke-test scheduling commands.",
        "Meerkat web_search is explicitly enabled for this live realtime session because the realtime model does not have native web search.",
        `This controller is a comms peer named ${controllerCommsName}; helpers should report back to that peer with send_message after delegate wiring succeeds.`,
        `Use these real mob-mcp tools for mob work: ${LIVE_CONTROLLER_VISIBLE_MOB_TOOLS.join(", ")}.`,
        `For delegate helpers that need shell/filesystem access, pass tooling {"mode":"inherit_parent"} or {"mode":"profile","source":{"type":"inline","model":"${workerModel}","tools":{"builtins":true,"shell":true,"comms":true}}}.`,
        `For mob_create, include a definition object. Minimal example: {"definition":{"id":"smoke-team","profiles":{"worker":{"model":"${workerModel}","tools":{"builtins":true,"shell":true,"comms":true},"runtime_mode":"autonomous_host"}}}}.`,
        "Do not use auto_wire_orchestrator as a substitute for delegate-to-controller wiring; it only applies to spawned mob members when an orchestrator member exists in the mob roster.",
        "Do not use mob_check_member in this smoke test. Delegate completion should be proven by peer comms, not by polling a status side channel.",
        `When constructing mob definitions, use ${workerModel} as the default model unless the user asks for another model.`,
      ],
    },
  );

  const channel = LiveChannel.session(client, session.id, {
    transport: "webrtc",
    turningMode: body.turningMode ?? "provider_managed",
  });
  const openResult = await channel.open();
  if (!isLiveWebrtcBootstrap(openResult.transport)) {
    throw new Error(`live/open returned ${openResult.transport.transport}, expected webrtc`);
  }
  activeLive = {
    sessionId: session.id,
    channel,
    channelId: openResult.channel_id,
    transport: openResult.transport,
    model,
    workerModel,
    notes: [],
    textOutputs: [],
  };

  return {
    session_id: session.id,
    channel_id: openResult.channel_id,
    transport: openResult.transport,
    capabilities: openResult.capabilities,
    continuity: openResult.continuity,
    tools: [...CALLBACK_TOOL_DEFS.map((tool) => tool.name), ...LIVE_CONTROLLER_VISIBLE_MOB_TOOLS],
  };
}

async function answerWebrtc(body: WebrtcAnswerRequest): Promise<unknown> {
  const { client } = await ensureRuntime();
  return client.liveWebrtcAnswer(body);
}

async function liveControl(channelId: string, action: string, body: Record<string, unknown>): Promise<unknown> {
  const { client } = await ensureRuntime();
  const live = activeLive;
  if (!live || live.channelId !== channelId) {
    throw Object.assign(new Error("Unknown or inactive live channel"), { statusCode: 404 });
  }
  if (action === "commit") {
    await client.liveCommitInput({
      channel_id: channelId,
      response_modality: asWireModality(body.response_modality),
    });
    return { ok: true };
  }
  if (action === "interrupt") {
    await client.liveInterrupt({ channel_id: channelId });
    return { ok: true };
  }
  if (action === "truncate") {
    await client.liveTruncate({
      channel_id: channelId,
      item_id: String(body.item_id ?? body.response_id ?? ""),
      content_index: Number(body.content_index ?? 0),
      audio_played_ms: Number(body.audio_played_ms ?? body.audio_end_ms ?? 0),
    });
    return { ok: true };
  }
  if (action === "refresh") {
    return client.liveRefresh({ channel_id: channelId });
  }
  if (action === "text") {
    const text = String((body as TextInputRequest).text ?? "").trim();
    if (!text) throw Object.assign(new Error("text is required"), { statusCode: 400 });
    await client.liveSendInput({
      channel_id: channelId,
      chunk: { kind: "text", text },
    });
    if ((body as TextInputRequest).commit) {
      await client.liveCommitInput({
        channel_id: channelId,
        response_modality: asWireModality((body as TextInputRequest).responseModality),
      });
    }
    return { ok: true };
  }
  if (action === "close") {
    await live.channel.close();
    activeLive = undefined;
    return { ok: true };
  }
  throw Object.assign(new Error(`Unknown live action: ${action}`), { statusCode: 404 });
}

async function apiState(): Promise<unknown> {
  const mobs = await pollMobs();
  const live = activeLive;
  return {
    active: Boolean(live),
    session_id: live?.sessionId,
    channel_id: live?.channelId,
    model: live?.model,
    worker_model: live?.workerModel,
    notes: live?.notes ?? [],
    text_outputs: live?.textOutputs ?? [],
    mobs,
  };
}

function parseArgs(): { port: number } {
  const portArg = process.argv.find((arg) => arg.startsWith("--port="));
  const port = Number(portArg?.slice("--port=".length) ?? process.env.PORT ?? 4173);
  return { port: Number.isFinite(port) && port > 0 ? port : 4173 };
}

async function readJson<T>(req: IncomingMessage): Promise<T> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  if (chunks.length === 0) return {} as T;
  return JSON.parse(Buffer.concat(chunks).toString("utf8")) as T;
}

function sendJson(res: ServerResponse, statusCode: number, value: unknown): void {
  const body = JSON.stringify(value);
  res.writeHead(statusCode, {
    "content-type": "application/json; charset=utf-8",
    "content-length": Buffer.byteLength(body),
  });
  res.end(body);
}

function sendError(res: ServerResponse, error: unknown): void {
  const statusCode =
    typeof error === "object" && error !== null && "statusCode" in error
      ? Number((error as { statusCode?: unknown }).statusCode)
      : 500;
  sendJson(res, Number.isFinite(statusCode) ? statusCode : 500, {
    error: error instanceof Error ? error.message : String(error),
  });
}

function contentType(pathname: string): string {
  switch (extname(pathname)) {
    case ".css":
      return "text/css; charset=utf-8";
    case ".js":
      return "text/javascript; charset=utf-8";
    case ".html":
      return "text/html; charset=utf-8";
    default:
      return "application/octet-stream";
  }
}

async function serveStatic(req: IncomingMessage, res: ServerResponse, pathname: string): Promise<void> {
  const requested = pathname === "/" ? "/index.html" : pathname;
  const normalized = normalize(decodeURIComponent(requested)).replace(/^(\.\.[/\\])+/, "");
  const filePath = resolve(PUBLIC_ROOT, `.${normalized}`);
  if (!filePath.startsWith(PUBLIC_ROOT) || relative(PUBLIC_ROOT, filePath).startsWith("..")) {
    res.writeHead(403).end("forbidden");
    return;
  }
  if (!existsSync(filePath) || !(await stat(filePath)).isFile()) {
    res.writeHead(404).end("not found");
    return;
  }
  res.writeHead(200, { "content-type": contentType(filePath) });
  createReadStream(filePath).pipe(res);
  req.resume();
}

async function route(req: IncomingMessage, res: ServerResponse): Promise<void> {
  const url = new URL(req.url ?? "/", "http://127.0.0.1");
  if (req.method === "POST" && url.pathname === "/api/start") {
    return sendJson(res, 200, await startLive(await readJson<StartRequest>(req)));
  }
  if (req.method === "POST" && url.pathname === "/api/webrtc/answer") {
    return sendJson(res, 200, await answerWebrtc(await readJson<WebrtcAnswerRequest>(req)));
  }
  if (req.method === "GET" && url.pathname === "/api/state") {
    return sendJson(res, 200, await apiState());
  }
  const control = url.pathname.match(/^\/api\/live\/([^/]+)\/([^/]+)$/);
  if (req.method === "POST" && control) {
    return sendJson(
      res,
      200,
      await liveControl(control[1], control[2], await readJson<Record<string, unknown>>(req)),
    );
  }
  if (req.method === "GET" || req.method === "HEAD") {
    return serveStatic(req, res, url.pathname);
  }
  res.writeHead(405).end("method not allowed");
}

const { port } = parseArgs();
const server = createServer((req, res) => {
  route(req, res).catch((error) => sendError(res, error));
});

server.listen(port, "127.0.0.1", () => {
  console.log(`Meerkat Live WebRTC smoke test: http://127.0.0.1:${port}/`);
  console.log("Build rkat-rpc with `--features live-webrtc` and set RKAT_RPC if it is not on PATH.");
  void ensureRuntime()
    .then(() => console.log("rkat-rpc runtime warmed; Start will go straight to live/open."))
    .catch((error) => console.warn(`rkat-rpc warmup failed; Start will retry: ${error}`));
});

for (const signal of ["SIGINT", "SIGTERM"] as const) {
  process.once(signal, async () => {
    if (activeLive) {
      await activeLive.channel.close().catch(() => undefined);
    }
    server.close();
    process.exit(signal === "SIGINT" ? 130 : 0);
  });
}
