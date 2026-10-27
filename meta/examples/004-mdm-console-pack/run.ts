import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import YAML from "yaml";

import {
  Meta,
  type MobHandle,
  type AgentBuildContext,
  type AgentBuildDraft,
  type AgentCustomizer,
  type DurableAgentSpec,
  type ManagedPeerEdge,
  type RosterProvider,
  type TopologyProvider,
} from "../../sdk/typescript/src/index.ts";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../..");
const configDir = join(here, "config");
const scenarioPath = join(here, "scenario.yaml");

type ScenarioTarget = {
  id: string;
  name: string;
  site: string;
  platform: string;
  labels?: Record<string, string>;
};

type Scenario = {
  scenario_id: string;
  default_operator: string;
  console_expected_title: string;
  targets: ScenarioTarget[];
  links: Array<[string, string]>;
};

type RemoteTargetBinding = {
  id: string;
  name?: string;
  site?: string;
  platform?: string;
  address?: string;
  public_key?: string;
  bootstrap_token?: string;
  binding?: Record<string, unknown>;
  labels?: Record<string, string>;
};

type Args = Record<string, string | boolean>;

const scenario = YAML.parse(readFileSync(scenarioPath, "utf8")) as Scenario;

function parseArgs(argv = process.argv.slice(2)): Args {
  const result: Args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!arg.startsWith("--")) continue;
    const key = arg.slice(2);
    const next = argv[index + 1];
    if (next && !next.startsWith("--")) {
      result[key] = next;
      index += 1;
    } else {
      result[key] = true;
    }
  }
  return result;
}

function stringArg(args: Args, key: string): string | undefined {
  const value = args[key];
  return typeof value === "string" && value.trim() ? value.trim() : undefined;
}

function tomlString(value: string): string {
  return JSON.stringify(value);
}

function socketHost(socketAddress: string): string {
  if (socketAddress.startsWith("[")) {
    const end = socketAddress.indexOf("]");
    if (end > 0) return socketAddress.slice(1, end);
  }
  const colon = socketAddress.lastIndexOf(":");
  return colon >= 0 ? socketAddress.slice(0, colon) : socketAddress;
}

function isUnspecifiedHost(host: string): boolean {
  return host === "0.0.0.0" || host === "::" || host === "[::]";
}

function defaultSupervisorAdvertisedAddress(bindAddress: string): string {
  const host = socketHost(bindAddress);
  if (isUnspecifiedHost(host)) {
    throw new Error(
      "MDM supervisor bridge bind address is unspecified; set --supervisor-advertised tcp://<console-reachable-host>:<port> or MDM_SUPERVISOR_ADVERTISED_ADDRESS",
    );
  }
  return `tcp://${bindAddress}`;
}

function writeRuntimeMobConfig(
  args: Args,
  stateDir: string,
): {
  path: string;
  bindAddress: string;
  advertisedAddress: string;
} {
  const bindAddress =
    stringArg(args, "supervisor-bind") ??
    process.env.MDM_SUPERVISOR_BIND_ADDRESS ??
    "127.0.0.1:5790";
  const advertisedAddress =
    stringArg(args, "supervisor-advertised") ??
    process.env.MDM_SUPERVISOR_ADVERTISED_ADDRESS ??
    defaultSupervisorAdvertisedAddress(bindAddress);
  const source = readFileSync(join(configDir, "mob.toml"), "utf8");
  const path = join(stateDir, "mob.generated.toml");
  const content = `${source.trimEnd()}

[backend.external.supervisor_bridge]
bind_address = ${tomlString(bindAddress)}
advertised_address = ${tomlString(advertisedAddress)}
`;
  writeFileSync(path, content);
  return { path, bindAddress, advertisedAddress };
}

function repoCargoEnv(): Record<string, string> {
  const result = spawnSync(
    join(repoRoot, "scripts/repo-cargo"),
    ["--print-env"],
    {
      cwd: repoRoot,
      encoding: "utf8",
    },
  );
  if (result.status !== 0) {
    throw new Error(`repo-cargo --print-env failed:\n${result.stderr}`);
  }
  return Object.fromEntries(
    result.stdout
      .trim()
      .split(/\n/)
      .map((line) => line.split("=", 2))
      .filter(([key, value]) => key && value),
  );
}

function ensureGatewayBin(skipBuild: boolean): string {
  if (process.env.META_RPC_GATEWAY_BIN)
    return process.env.META_RPC_GATEWAY_BIN;
  const env = repoCargoEnv();
  const gateway = join(env.CARGO_TARGET_DIR, "debug", "rpc_gateway");
  if (!skipBuild || !existsSync(gateway)) {
    const result = spawnSync(
      join(repoRoot, "scripts/repo-cargo"),
      ["build", "-p", "meerkat-meta", "--bin", "rpc_gateway"],
      { cwd: repoRoot, stdio: "inherit" },
    );
    if (result.status !== 0) throw new Error("failed to build rpc_gateway");
  }
  return gateway;
}

function readTargetBindings(args: Args): RemoteTargetBinding[] {
  const explicitPath =
    typeof args.targets === "string"
      ? args.targets
      : process.env.MDM_REMOTE_TARGETS_FILE;
  const raw =
    process.env.MDM_REMOTE_TARGETS_JSON ??
    (explicitPath ? readFileSync(resolve(explicitPath), "utf8") : "[]");
  const parsed = JSON.parse(raw) as unknown;
  const entries = Array.isArray(parsed) ? parsed : [parsed];
  return entries.map((target, index) => {
    if (!target || typeof target !== "object") {
      throw new Error(`remote target ${index} must be an object`);
    }
    const value = target as RemoteTargetBinding;
    if (!value.id) throw new Error(`remote target ${index} is missing id`);
    return value;
  });
}

function bindingFor(target: RemoteTargetBinding): Record<string, unknown> {
  if (target.binding) return target.binding;
  if (!target.address || !target.public_key) {
    throw new Error(
      `remote target ${target.id} must provide either binding or address + public_key`,
    );
  }
  return {
    kind: "external",
    address: target.address,
    bootstrap_token: target.bootstrap_token,
    identity: {
      kind: "ed25519_public_key",
      public_key: target.public_key,
    },
  };
}

function scenarioTarget(id: string): ScenarioTarget | undefined {
  return scenario.targets.find((target) => target.id === id);
}

function targetLabels(target: RemoteTargetBinding): Record<string, string> {
  const scenarioEntry = scenarioTarget(target.id);
  return {
    ...(scenarioEntry?.labels ?? {}),
    ...(target.labels ?? {}),
    console_group: "Managed Targets",
    durable_identity: target.id,
    site: target.site ?? scenarioEntry?.site ?? "remote",
    platform: target.platform ?? scenarioEntry?.platform ?? "unknown",
    transport: "mob_remote",
    claim_state: "remote",
  };
}

class MdmRosterProvider implements RosterProvider {
  constructor(private readonly targets: RemoteTargetBinding[]) {}

  async roster(): Promise<DurableAgentSpec[]> {
    const targetSpecs: DurableAgentSpec[] = this.targets.map((target) => {
      const scenarioEntry = scenarioTarget(target.id);
      const displayName = target.name ?? scenarioEntry?.name ?? target.id;
      return {
        identity: target.id,
        profile: "target",
        addressability: "addressable",
        displayName,
        labels: {
          ...targetLabels(target),
          display_name: displayName,
        },
        context: null,
        additionalInstructions: [],
        runtimeModeOverride: "turn_driven",
        backend: "external",
        binding: bindingFor(target),
      };
    });
    return [
      {
        identity: "hive",
        profile: "hive",
        addressability: "addressable",
        displayName: "Hive",
        labels: {
          console_group: "Fleet Control",
          site: "all",
          platform: "meta",
          claim_state: "coordinator",
          durable_identity: "hive",
        },
        context: null,
        additionalInstructions: [
          "Targets are real mob peers, not records in a target registry.",
          "When the operator asks targets a question, send the question to the target peers through comms and wait for their replies.",
          "Do not answer target machine questions from labels, metadata, or assumptions.",
        ],
      },
      ...targetSpecs,
    ];
  }
}

class MdmTopologyProvider implements TopologyProvider {
  constructor(private readonly targets: RemoteTargetBinding[]) {}

  async computeEdges(targetIdentities: string[]): Promise<ManagedPeerEdge[]> {
    const identities = new Set(targetIdentities);
    const edges: ManagedPeerEdge[] = [];
    const add = (a: string, b: string) => {
      if (identities.has(a) && identities.has(b) && a !== b)
        edges.push({ a, b });
    };
    for (const [a, b] of scenario.links) add(a, b);
    for (const target of this.targets) add("hive", target.id);
    return edges;
  }
}

class MdmCustomizer implements AgentCustomizer {
  async customizeBuild(
    _context: AgentBuildContext,
    spec: DurableAgentSpec,
    draft: AgentBuildDraft,
  ): Promise<void> {
    draft.labels = {
      ...draft.labels,
      ...spec.labels,
      sdk_toolbelt: "mdm-mob-roster",
    };
    draft.additionalInstructions.push(...spec.additionalInstructions);
  }
}

async function getConsoleTitle(baseUrl: string): Promise<string | undefined> {
  const response = await fetch(`${baseUrl}/console/experience`);
  if (!response.ok)
    throw new Error(`/console/experience returned ${response.status}`);
  const experience = (await response.json()) as {
    console_config?: { title?: string };
  };
  return experience.console_config?.title;
}

let consoleRpcSequence = 0;

async function consoleRpc<T>(
  baseUrl: string,
  method: string,
  params: Record<string, unknown>,
): Promise<T> {
  consoleRpcSequence += 1;
  const response = await fetch(`${baseUrl}/console/rpc`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: `mdm-${consoleRpcSequence}`,
      method,
      params,
    }),
  });
  if (!response.ok)
    throw new Error(`${method} returned HTTP ${response.status}`);
  const payload = (await response.json()) as {
    result?: T;
    error?: { code?: number; message?: string };
  };
  if (payload.error) {
    throw new Error(
      `${method} RPC error ${String(payload.error.code)}: ${payload.error.message ?? "unknown error"}`,
    );
  }
  return payload.result as T;
}

async function waitForConsoleTerminal(
  baseUrl: string,
  targetId: string,
  interactionId: string,
): Promise<Record<string, unknown>> {
  const deadline = Date.now() + 180_000;
  while (Date.now() < deadline) {
    const page = await consoleRpc<{
      frames?: Array<{
        interaction_id?: string;
        kind?: string;
        payload?: Record<string, unknown>;
      }>;
    }>(baseUrl, "meta/console/query_timeline", {
      mode: "recent",
      identity: targetId,
      limit: 400,
    });
    const frames = page.frames ?? [];
    const failed = frames.find(
      (frame) =>
        frame.interaction_id === interactionId &&
        (frame.kind === "interaction_failed" ||
          frame.kind === "message_delivery_failed"),
    );
    if (failed) {
      // Delivery failure projection can append the pending-interaction
      // terminal just before the typed error frame. Give that frame one poll
      // interval to land, then report the complete correlated failure set.
      await new Promise((resolve) => setTimeout(resolve, 250));
      const failurePage = await consoleRpc<{
        frames?: Array<{
          interaction_id?: string;
          kind?: string;
          payload?: Record<string, unknown>;
        }>;
      }>(baseUrl, "meta/console/query_timeline", {
        mode: "recent",
        identity: targetId,
        limit: 400,
      });
      const correlated = (failurePage.frames ?? []).filter(
        (frame) => frame.interaction_id === interactionId,
      );
      throw new Error(
        `${targetId} interaction failed: ${JSON.stringify(correlated)}`,
      );
    }
    const completed = frames.find(
      (frame) =>
        frame.interaction_id === interactionId &&
        frame.kind === "interaction_complete",
    );
    if (completed) {
      const terminal = completed.payload ?? {};
      if (Object.keys(terminal).length === 0) {
        throw new Error(`${targetId} returned an empty terminal response`);
      }
      return terminal;
    }
    await new Promise((resolve) => setTimeout(resolve, 1_000));
  }
  throw new Error(
    `timed out waiting for ${targetId} terminal response (${interactionId})`,
  );
}

async function waitForVerifiedHiveTerminal(
  baseUrl: string,
  targetIds: string[],
  responseMarker: string,
): Promise<Record<string, unknown>> {
  const deadline = Date.now() + 180_000;
  const incomplete =
    /\b(no (?:real )?(?:peer )?repl(?:y|ies)|not contacted|not received|unknown|could not|couldn't|cannot|can't|missing|only|invalid|null|does not|still|follow-up|without)\b/i;
  while (Date.now() < deadline) {
    const page = await consoleRpc<{
      frames?: Array<{
        kind?: string;
        payload?: Record<string, unknown>;
      }>;
    }>(baseUrl, "meta/console/query_timeline", {
      mode: "recent",
      identity: "hive",
      limit: 400,
    });
    for (const frame of page.frames ?? []) {
      if (frame.kind !== "interaction_complete") continue;
      const payload = frame.payload ?? {};
      const response =
        typeof payload.result === "string"
          ? payload.result
          : JSON.stringify(payload.result);
      const markerCount = response.split(responseMarker).length - 1;
      if (
        markerCount >= targetIds.length &&
        targetIds.every((targetId) => response.includes(targetId)) &&
        !incomplete.test(response)
      ) {
        return payload;
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 1_000));
  }
  throw new Error(
    `timed out waiting for hive to publish verified target replies (${responseMarker})`,
  );
}

async function runRealTargetSmoke(
  handle: MobHandle,
  targets: RemoteTargetBinding[],
  baseUrl: string,
  waitForTerminal: boolean,
): Promise<void> {
  if (targets.length === 0) {
    throw new Error("real target smoke requires at least one target binding");
  }
  const members = await handle.listMembers();
  const activeMembers = new Set(members.map((member) => member.agentIdentity));
  for (const target of targets) {
    if (!activeMembers.has(target.id)) {
      throw new Error(
        `real target smoke expected active mob member '${target.id}', got: ${[...activeMembers].join(", ")}`,
      );
    }
  }

  for (const target of targets) {
    const prompt = [
      "MDM real-target smoke.",
      "This is a peer turn delivered through the Meta/Meerkat mob path.",
      "Inspect the local target host before answering.",
      "Report hostname, OS/kernel, current user, and whether shell tools are available.",
      "Do not answer from roster labels or binding metadata.",
    ].join(" ");
    const result = await consoleRpc<{
      interaction_id: string;
      session_id?: string;
      status: string;
    }>(baseUrl, "meta/console/send", {
      identity: target.id,
      content: prompt,
      origin: "mdm-real-target-smoke",
      idempotency_key: `mdm-real-target-smoke-${target.id}`,
      handling_mode: "queue",
    });
    if (!result.interaction_id) {
      throw new Error(
        `real target smoke did not reserve an interaction for ${target.id}`,
      );
    }
    const sessionLabel = result.session_id || "peer-only";
    console.log(
      `[mdm-real-target-smoke] ${target.id}: accepted interaction=${result.interaction_id} session=${sessionLabel}`,
    );
  }

  if (waitForTerminal) {
    const targetList = targets.map((target) => target.id).join(", ");
    const responseMarker = "MDM_E2E_TARGET_RESPONSE_OK";
    const result = await consoleRpc<{
      interaction_id: string;
      session_id?: string;
      status: string;
    }>(baseUrl, "meta/console/send", {
      identity: "hive",
      content: [
        `Contact these managed targets through peer comms: ${targetList}.`,
        targets.length === 1
          ? `This fixture has exactly one visible managed peer; its cryptographic route is the durable target ${targets[0].id}.`
          : "Match each visible managed peer to the requested target before sending.",
        "For each target, use the correlated send_request tool (not send_message) with the supported intent checksum_token and handling_mode queue.",
        `Ask it to inspect its local host, report hostname, OS/kernel, current user, and whether shell tools are available, and include the exact marker ${responseMarker}.`,
        "Wait for every correlated terminal response, then return a concise per-target summary preserving that marker.",
        "Do not infer target facts from roster labels or binding metadata.",
      ].join(" "),
      origin: "mdm-real-target-e2e",
      idempotency_key: `mdm-real-target-e2e-${targetList}`,
      handling_mode: "queue",
    });
    if (!result.interaction_id) {
      throw new Error("real target E2E did not reserve a hive interaction");
    }
    console.log(
      `[mdm-real-target-e2e] hive: accepted interaction=${result.interaction_id}`,
    );
    const terminal = await waitForConsoleTerminal(
      baseUrl,
      "hive",
      result.interaction_id,
    );
    const verifiedTerminal = await waitForVerifiedHiveTerminal(
      baseUrl,
      targets.map((target) => target.id),
      responseMarker,
    );
    console.log(
      `[mdm-real-target-e2e] hive: dispatch response=${JSON.stringify(terminal)}`,
    );
    console.log(
      `[mdm-real-target-e2e] hive: verified target response=${JSON.stringify(verifiedTerminal)}`,
    );
  }
}

async function main() {
  const args = parseArgs();
  const skipBuild = Boolean(args["skip-build"]);
  const smoke = Boolean(args.smoke);
  const realTargetE2e = Boolean(args["real-target-e2e"]);
  const realTargetSmoke = Boolean(args["real-target-smoke"]) || realTargetE2e;
  const wait = Boolean(args.wait) || Boolean(args["browser-smoke"]);
  const hiveKickoff = stringArg(args, "hive-kickoff");
  const useDemoLlm =
    Boolean(args["demo-llm"]) ||
    (!Boolean(args["real-llm"]) && !process.env.OPENAI_API_KEY);
  const targets = readTargetBindings(args);
  if (targets.length === 0 && !Boolean(args["allow-empty-targets"])) {
    throw new Error(
      "no remote targets configured; pass --targets <json> or set MDM_REMOTE_TARGETS_JSON",
    );
  }

  const stateDir = stringArg(args, "state-dir") ?? join(here, ".state");
  mkdirSync(stateDir, { recursive: true });
  const mobConfig = writeRuntimeMobConfig(args, stateDir);

  let builder = Meta.builder()
    .mob(mobConfig.path)
    .gateway(ensureGatewayBin(skipBuild))
    .consoleConfig(join(configDir, "console.toml"))
    .consoleAuthRequired(false)
    .consoleFetchTimeoutMs(120_000)
    .memberCommsTcp(process.env.MDM_MEMBER_COMMS_ADDRESS ?? "127.0.0.1:0")
    .persistentState(stateDir)
    .rosterProvider(new MdmRosterProvider(targets))
    .topologyProvider(new MdmTopologyProvider(targets))
    .agentCustomizer(new MdmCustomizer());
  if (useDemoLlm) builder = builder.demoLlm();
  const runtime = await builder.build();

  try {
    const handle = runtime.mobHandle();
    await handle.setMobLabels({
      example_pack: "004-mdm-console",
      scenario: scenario.scenario_id,
      remote_targets: String(targets.length),
      remote_topology: "mob-roster",
      supervisor_bridge: mobConfig.advertisedAddress,
    });
    const edgeReport = await handle.reconcileEdges();
    console.log(`[mdm] topology: ${JSON.stringify(edgeReport)}`);
    const baseUrl = runtime.rustHttpBaseUrl;
    if (!baseUrl)
      throw new Error("Meta runtime did not expose an HTTP console URL");
    console.log(`[mdm] console: ${baseUrl}/console`);
    console.log(`[mdm] remote-targets: ${targets.length}`);
    console.log(`[mdm] supervisor-bridge: ${mobConfig.advertisedAddress}`);

    if (smoke || realTargetSmoke) {
      const title = await getConsoleTitle(baseUrl);
      if (title !== scenario.console_expected_title) {
        throw new Error(`unexpected console title: ${String(title)}`);
      }
    }
    if (realTargetSmoke) {
      await runRealTargetSmoke(handle, targets, baseUrl, realTargetE2e);
      console.log("[mdm-real-target-smoke] ok");
      if (realTargetE2e) console.log("[mdm-real-target-e2e] ok");
    }
    if (smoke) {
      console.log("[mdm-smoke] ok");
      return;
    }
    if (hiveKickoff) {
      console.log("[mdm] sending hive kickoff");
      await runtime.send("hive", hiveKickoff);
    }
    if (wait) {
      await new Promise<void>((resolve) => {
        process.once("SIGINT", resolve);
        process.once("SIGTERM", resolve);
      });
    }
  } finally {
    await runtime.shutdown();
  }
}

main().catch((error) => {
  console.error(error instanceof Error ? error.stack : error);
  process.exitCode = 1;
});
