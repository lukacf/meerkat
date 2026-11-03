// Opt-in installed native journey. Run explicitly; this is not an inert test.
// The selected Meta SDK owns the only runtime, reader, admission and shutdown.
import { createHash } from "node:crypto";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { constants, createReadStream, closeSync, lstatSync, mkdirSync, openSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const PROMPT = "Return the exact installed TypeScript native acknowledgement.";
const ANSWER = "Installed TypeScript native acknowledgement.";
const canonical = value => JSON.stringify(value);
const sha = value => createHash("sha256").update(value).digest("hex");
class FixtureFailure extends Error { constructor(label) { super(label); this.label = label; } }
function require(value, label) { if (!value) throw new FixtureFailure(label); }
function failure(error, phase) {
  return { phase, error_type: error?.constructor?.name ?? "unknown",
    ...(error instanceof FixtureFailure ? { label: error.label } : {}) };
}
function save(path, value) { writeFileSync(path, canonical(value) + "\n", { mode: 0o600 }); }
async function verify(pin) {
  // An existing virtualenv executable can be a symlink. Retain its invocation
  // path and pin the explicitly selected real target without changing venv use.
  const path = pin.resolved_path ?? pin.path;
  if (pin.resolved_path) require(realpathSync(pin.path) === path, "selected input target changed");
  const before = lstatSync(path);
  require(before.isFile() && !before.isSymbolicLink(), "selected input is not a regular file");
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path, { flags: constants.O_RDONLY | constants.O_NOFOLLOW })) hash.update(chunk);
  const after = lstatSync(path);
  require(hash.digest("hex") === pin.sha256 && before.ino === after.ino && before.size === after.size
    && before.mtimeMs === after.mtimeMs && (!pin.resolved_path || realpathSync(pin.path) === path), "selected input changed");
}

const input = JSON.parse(readFileSync(process.argv[2], "utf8"));
require(input.schema === 1 && process.platform !== "win32" && Number(process.versions.node.split(".")[0]) >= 22,
  "installed TypeScript input or platform differs");
require(input.pair.core === "8006dba35a5a13d49bb605311143eef56df2ccd5"
  && input.pair.meta === "5cf626e8e4b7f4f01ad673c94e1630f28433f23b", "selected native pair differs");
for (const pin of input.files) await verify(pin);
for (const name of ["gateway", "python", "support", "verifier", "python_host_control", "meta_entry", "toolkit_entry",
  "meta_package", "toolkit_package", "meta_archive", "toolkit_archive", "installation_lock"]) {
  require(input.files.some(pin => pin.path === input[name].path && pin.sha256 === input[name].sha256), "required selected input is unpinned");
}
for (const [kind, name, entry] of [["meta", "meta-sdk", "index.js"], ["toolkit", "toolkit", "native-host.js"]]) {
  const root = join(input.installed_prefix, "node_modules", "@rkat", name);
  require(input[kind + "_package"].path === join(root, "package.json")
    && input[kind + "_entry"].path === join(root, "dist", entry)
    && realpathSync(root) === root, "SDK entry is not the selected isolated installed package");
  const metadata = JSON.parse(readFileSync(input[kind + "_package"].path, "utf8"));
  require(metadata.name === "@rkat/" + name && metadata.version === (kind === "meta" ? "0.9.0" : "0.1.0"),
    "selected installed SDK package differs");
}
require(input.installation_lock.path === join(input.installed_prefix, "package-lock.json"), "installed dependency resolution is unpinned");
require(resolve(input.output) === input.output, "fixture output must be an absolute fresh directory");
mkdirSync(input.output, { mode: 0o700 });
process.chdir(input.output);
const report = { status: "failed", scope: "actual installed TypeScript preparation/adoption/input/completion/shutdown; controlled loopback model and source; no acquisition or live provider claim",
  pair: input.pair, source: input.source,
  packages: { prefix: input.installed_prefix, meta: input.meta_archive, toolkit: input.toolkit_archive, lock: input.installation_lock },
  imports: {}, support_commands: [], requests: [], model_errors: [], transport_count: 0, init_count: 0 };
let phase = "imports", runtime, host, transport, server, modelEnabled = false;
let primary, cleanupFailure;
const nativeEnv = { PATH: "/usr/bin:/bin", LANG: "C.UTF-8", PYTHONPATH: input.python_roots.join(":"), PYTHONDONTWRITEBYTECODE: "1" };
const event = { occurrence: "ts-event-1", account: "ts-fixture", sender: "ts-requester", target: "original", content: PROMPT };
let eventReads = 0;

async function support(command, extra = {}) {
  const selected = { output: input.output, gateway: input.gateway, verifier: input.verifier,
    python_host_control: input.python_host_control, ...extra };
  const requestPath = join(input.output, command + "-input.json");
  const resultPath = join(input.output, command + "-result.json");
  save(requestPath, selected);
  const stdout = openSync(join(input.output, command + ".stdout"), "wx", 0o600);
  const stderr = openSync(join(input.output, command + ".stderr"), "wx", 0o600);
  let child;
  try { child = spawn(input.python.path, [input.support.path, command, requestPath, resultPath],
    { cwd: input.output, env: nativeEnv, detached: true, stdio: ["ignore", stdout, stderr] }); }
  finally { closeSync(stdout); closeSync(stderr); }
  const observation = { command, pid: child.pid ?? null, forced: false };
  report.support_commands.push(observation);
  save(join(input.output, "support-processes.json"), report.support_commands);
  const timer = setTimeout(() => {
    observation.forced = true;
    try { process.kill(-child.pid, "SIGKILL"); } catch { /* Record the joined child outcome below. */ }
  }, 180_000);
  const exited = await new Promise(resolveExit => {
    child.once("error", error => { observation.spawn_error_type = error.constructor.name; });
    child.once("close", (code, signal) => resolveExit({ code, signal }));
  });
  clearTimeout(timer);
  Object.assign(observation, exited);
  save(join(input.output, "support-processes.json"), report.support_commands);
  require(exited.code === 0 && exited.signal === null && !observation.forced, "artifact support did not exit normally");
  const result = JSON.parse(readFileSync(resultPath, "utf8"));
  require(result.status === "passed" && !result.finalization?.forced_artifact_cleanup, "artifact finalization did not settle");
  return result;
}

try {
  const Meta = await import(pathToFileURL(input.meta_entry.path).href);
  const Toolkit = await import(pathToFileURL(input.toolkit_entry.path).href);
  report.imports = { meta: resolve(input.meta_entry.path), toolkit: resolve(input.toolkit_entry.path) };
  // Native-host's instanceof check also requires the exact same loaded Meta
  // class identity. A second peer package cannot silently supply this builder.
  phase = "loopback_provider";
  server = createServer(async (request, response) => {
    try {
      if (request.method === "GET" && request.url === "/fixture-event") {
        require(++eventReads === 1, "controlled source occurrence was read twice");
        response.writeHead(200, { "Content-Type": "application/json" });
        response.end(canonical(event)); return;
      }
      require(request.method === "POST" && request.url === "/v1/chat/completions", "unexpected model endpoint");
      const chunks = []; let size = 0;
      for await (const chunk of request) {
        size += chunk.length; require(size <= 4 * 1024 * 1024, "model request exceeded bound"); chunks.push(chunk);
      }
      const value = JSON.parse(Buffer.concat(chunks).toString("utf8"));
      report.requests.push(value);
      save(join(input.output, "model-requests.json"), report.requests);
      require(modelEnabled && report.requests.length === 1 && value.model === "fixture-model"
        && value.messages.filter(row => row.role === "user" && row.content === PROMPT).length === 1,
        "unexpected model work or original input");
      if (value.stream === true) {
        response.writeHead(200, { "Content-Type": "text/event-stream" });
        for (const choice of [{ delta: { role: "assistant", content: ANSWER }, finish_reason: null },
          { delta: {}, finish_reason: "stop" }]) {
          response.write("data: " + canonical({ id: "ts-native-model-1", object: "chat.completion.chunk",
            created: 0, model: value.model, choices: [{ index: 0, ...choice }] }) + "\n\n");
        }
        response.end("data: [DONE]\n\n");
      } else {
        response.writeHead(200, { "Content-Type": "application/json" });
        response.end(canonical({ id: "ts-native-model-1", object: "chat.completion", created: 0, model: value.model,
          choices: [{ index: 0, message: { role: "assistant", content: ANSWER }, finish_reason: "stop" }],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } }));
      }
    } catch (error) {
      report.model_errors.push(failure(error, "controlled_provider"));
      response.writeHead(500, { "Content-Type": "application/json" }); response.end('{"error":"controlled fixture refusal"}');
    }
  });
  await new Promise((done, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", done); });
  const port = server.address().port;
  const specs = [Meta.parseDurableAgentSpec({ identity: "helper", profile: "research", runtime_mode_override: "turn_driven" }),
    Meta.parseDurableAgentSpec({ identity: "original", profile: "original", runtime_mode_override: "turn_driven" })];
  const roster = specs.map(Meta.durableAgentSpecToDict);
  phase = "artifact_preparation";
  const prepared = await support("prepare", { roster, model_port: port });
  await verify(prepared.plan);
  const preparedPlanBytes = readFileSync(prepared.plan.path);
  require(sha(preparedPlanBytes) === prepared.plan.sha256, "prepared plan changed before native storage selection");
  const preparedPlan = JSON.parse(preparedPlanBytes.toString("utf8"));
  report.prepared = { plan: prepared.plan, source: prepared.source, model_config: prepared.model_config };
  // The real transport inherits only the same clean environment as existing
  // installed Python journeys. Package imports have already resolved exactly.
  for (const key of Object.keys(process.env)) delete process.env[key];
  Object.assign(process.env, prepared.environment);
  let rosterCalls = 0, finalizerCalls = 0;
  const builder = Meta.Meta.builder().gateway(input.gateway.path).gatewayTimeoutMs(60_000).initDeadline(180_000)
    .persistentState(preparedPlan.application.realm.state_root).meerkatConfig(prepared.model_config.path)
    .httpListen("127.0.0.1:0").consoleAuthRequired(false).consoleReadOnly(true)
    .rosterProvider({ async roster(context) {
      require(context.mobDefinition !== null, "native preparation omitted its selected definition");
      rosterCalls++; return specs;
    } }).topologyProvider({ async computeEdges() { return []; } });
  // Observe the existing public factory and request method without replacing
  // transport, responses, callback dispatch, init or cleanup semantics.
  const register = builder.gatewayTransport.bind(builder);
  builder.gatewayTransport = factory => register(options => {
    report.transport_count++; transport = factory(options);
    const send = transport.sendAsync.bind(transport);
    transport.sendAsync = async (request, options) => {
      if (request.method === "meta/init") report.init_count++;
      return await send(request, options);
    };
    return transport;
  });
  host = new Toolkit.ToolkitApplicationPreparation({ plan: prepared.plan }, async manifest => {
    require(++finalizerCalls === 1 && report.transport_count === 1 && transport.isRunning(), "finalizer lost its original owner");
    report.completed_manifest = manifest;
    const result = await support("finalize", { manifest });
    report.selection = result.selection;
    require(transport.isRunning(), "native owner exited during artifact finalization");
    return result.selection;
  }, { timeoutMs: 180_000 });
  phase = "native_preparation_adoption";
  runtime = await host.build(builder);
  require(runtime.isRunning && transport.isRunning() && report.transport_count === 1 && report.init_count === 1
    && finalizerCalls === 1 && rosterCalls > 0 && host.settlement.native.value === runtime
    && host.settlement.finalization.status === "fulfilled", "preparation did not adopt on the same runtime");
  report.preparation = { native: host.settlement.native.status, finalization: host.settlement.finalization.status,
    same_runtime: host.settlement.native.value === runtime, same_meta_class: true, roster_calls: rosterCalls };
  phase = "native_component_open";
  report.component = await host.hostControl.openComponent("ts-source");
  require(report.component.component === "ts-source" && report.component.state === "open", "native source component did not open");
  const helperBefore = await runtime.status("helper");
  require(helperBefore.identity === "helper" && helperBefore.sessionId, "unrelated native identity is absent");
  phase = "source_retention";
  const eventResponse = await fetch(`http://127.0.0.1:${port}/fixture-event`);
  require(eventResponse.status === 200, "owned source observation failed");
  const observed = await eventResponse.json();
  require(canonical(observed) === canonical(event), "controlled source occurrence changed");
  const authentication = { resource_id: "ts-fixture:ts-requester", revision: "ts-auth-v1",
    sha256: sha(canonical({ account: observed.account, sender: observed.sender, target: observed.target })) };
  const selectedEvent = { route: "ts-original", original: "ts-original-1", occurrence: observed.occurrence,
    original_authentication: authentication,
    root_event: { resource_id: observed.occurrence, revision: "ts-event-v1", sha256: sha(canonical(observed)) },
    contributors: [], content_sha256: sha(observed.content) };
  report.event = selectedEvent;
  report.retained = await host.hostControl.retainAuthenticatedEvent(selectedEvent, { timeoutMs: 60_000 });
  phase = "wrong_content_refusal";
  let refusal;
  try { await host.hostControl.submitAuthenticatedOriginal(report.retained.original, PROMPT + " changed", authentication, { timeoutMs: 60_000 }); }
  catch (error) { refusal = error; }
  // Native compares retained content before admission. This exact refusal is
  // not a timeout, an authority-bearing display string, or a retried submission.
  require(refusal?.refusal === "source_conflict" && report.requests.length === 0, "native did not refuse changed retained content");
  report.refusal = { kind: refusal.refusal, model_requests: report.requests.length };
  phase = "native_admission";
  modelEnabled = true;
  report.admission = await host.hostControl.submitAuthenticatedOriginal(report.retained.original, observed.content, authentication, { timeoutMs: 60_000 });
  const admission = report.admission;
  require(admission.state === "admitted" && admission.deduplicated === false && admission.turn_ticket
    && admission.source_settlement === "fresh_acceptance_promoted", "original was not freshly admitted and tracked");
  report.identity = await runtime.status("original");
  require(report.identity.identity === "original" && report.identity.sessionId === admission.session_id, "admitted session lost its exact identity");
  phase = "native_completion";
  const completed = await runtime.waitForTurn("original", admission.turn_ticket, { timeoutMs: 60_000 });
  report.completion = await runtime.turnResult("original", admission.turn_ticket);
  for (const result of [completed, report.completion]) require(result.identity === "original" && result.ticket === admission.turn_ticket
    && result.state === "completed" && result.outputStatus === "text" && result.output === ANSWER
    && result.outputTruncated === false, "exact native ticket did not complete with its own result");
  const helperAfter = await runtime.status("helper");
  require(helperAfter.identity === helperBefore.identity && helperAfter.sessionId === helperBefore.sessionId
    && helperAfter.agentRuntimeId === helperBefore.agentRuntimeId && helperAfter.generation === helperBefore.generation,
    "original admission changed the unrelated identity");
  report.unrelated_identity = { before: helperBefore, after: helperAfter };
  require(eventReads === 1 && report.requests.length === 1 && report.model_errors.length === 0, "controlled occurrence or model work repeated");
  report.status = "passed";
} catch (error) {
  primary = error; report.failure = failure(error, phase);
} finally {
  phase = "native_shutdown";
  try {
    if (runtime) await runtime.shutdown();
    else if (primary?.cleanup) await primary.cleanup.shutdown();
    report.termination = runtime?.lastGatewayTermination() ?? primary?.cleanup?.lastTermination() ?? null;
    report.shutdown_joined = Boolean(runtime) && !runtime.isRunning && !transport.isRunning();
    if (runtime) require(report.shutdown_joined && report.termination?.state === "exited"
      && report.termination.exitCode === 0 && report.termination.signal === null
      && report.termination.escalation === "none" && report.termination.nativeShutdown === "acknowledged",
      "same runtime shutdown did not acknowledge and join normally");
  } catch (error) { cleanupFailure = error; report.cleanup_failure = failure(error, phase); report.status = "failed"; }
  if (server) {
    server.closeIdleConnections();
    await new Promise(done => server.close(done));
    report.provider_joined = true;
  }
  if (report.model_errors.length) report.status = "failed";
  report.requests_count = report.requests.length;
  delete report.requests;
  save(join(input.output, "result.json"), report);
}
process.exitCode = primary || cleanupFailure || report.status !== "passed" ? 1 : 0;
