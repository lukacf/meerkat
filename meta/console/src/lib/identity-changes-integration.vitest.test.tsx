/**
 * Long-term agents in the stock console: a person creates, edits, archives,
 * unarchives and removes an agent from the roster, through the console's
 * commands of the `meta/identity/*` family. The scripted family
 * (fixtures/scripted-identity-family.cjs) answers every command in the
 * wire's own shapes. What a viewer is offered is the gateway's answer: the
 * experience read's `identity_changes` section and each agent's two
 * affordances.
 */
import React from "react";
import { act, cleanup, fireEvent, render, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createScriptedIdentityFamily } from "../../fixtures/scripted-identity-family.cjs";
import { ConsoleApp } from "../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "./contract";
import { consoleCommandMethod, type MetaConsoleTransport } from "./headless";

const SCOPE = "identity-changes-principal";

type Family = ReturnType<typeof createScriptedIdentityFamily>;
type View = ReturnType<typeof render>;

interface Harness extends MetaConsoleTransport {
  commands: Array<{ command: string; method: string; params: Record<string, unknown> }>;
}

/** A console transport over the scripted family: the experience read
 * carries its agents, its section and their affordances, and each command
 * of the family is answered as the console RPC answers it. With
 * `serveFamily` false the gateway serves no method of the family. */
function transport(family: Family, { serveFamily = true, readOnly = false }: { serveFamily?: boolean; readOnly?: boolean } = {}): Harness {
  const harness: Harness = {
    commands: [],
    loadExperience: async () => ({
      contract_version: "test",
      runtime_id: "identity-changes-runtime",
      storage_scope: SCOPE,
      console_config: {},
      console_policy: readOnly ? { read_only: true } : {},
      runtime_capabilities: { can_spawn_members: false, can_send_messages: true, can_retire_members: true },
      agent_sidebar: { live_snapshot: { agents: family.experienceAgents() } },
      identity_changes: family.experienceSection(),
      activity_feed: { filter_presets: [], active_preset_id: "all" },
    }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({
      version: "test",
      methods: serveFamily
        ? [...new Set([...Object.values(CONSOLE_RPC_METHODS), ...family.methods])]
        : Object.values(CONSOLE_RPC_METHODS).filter((method) => !method.startsWith("meta/identity/")),
    }) as never,
    queryTimeline: async () => ({ frames: [], available: true }) as never,
    subscribeTimeline: () => () => undefined,
    send: async (input) => ({ interaction_id: "sent", identity: input.identity, cursor: "console:x" }) as never,
    executeCommand: async (input) => {
      const method = consoleCommandMethod(input.command);
      const params = (input.params || {}) as Record<string, unknown>;
      harness.commands.push({ command: input.command, method, params });
      // The requester's own held changes, as the decisions read lists them.
      if (method === CONSOLE_RPC_METHODS.decisionsList) {
        return { command: input.command, accepted: true, result: { decisions: [], statuses: family.decisionStatuses(), unreadable: [] } } as never;
      }
      const answer = family.handle(method, params);
      if (!answer) return { command: input.command, accepted: true, result: {} } as never;
      if (answer.lost) throw new Error(`${method} request timed out`);
      if (answer.error) {
        throw Object.assign(new Error(`${method} RPC error: ${answer.error.message}`), { rpcError: answer.error });
      }
      return { command: input.command, accepted: true, result: answer.result } as never;
    },
  };
  return harness;
}

function seedDock(target: Record<string, unknown>): void {
  window.localStorage.setItem(`meta-console-dock-state:${JSON.stringify(["", SCOPE])}`, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
    panels: [{ id: "panel-1", mode: "console", target }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-1",
  }));
}

const ROSTER = { id: "roster", kind: "roster", title: "Roster" };

async function openRoster(family: Family, options?: { serveFamily?: boolean; readOnly?: boolean }): Promise<{ view: View; harness: Harness }> {
  seedDock(ROSTER);
  const harness = transport(family, options);
  const view = render(<ConsoleApp baseUrl="" transport={harness} />);
  await view.findByTestId("roster-panel", undefined, { timeout: 5_000 });
  return { view, harness };
}

async function settle(ms: number): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

function present(view: View, testId: string): boolean {
  return view.queryByTestId(testId) !== null;
}

async function find(view: View, testId: string): Promise<HTMLElement> {
  return view.findByTestId(testId, undefined, { timeout: 5_000 });
}

async function gone(view: View, testId: string): Promise<void> {
  await waitFor(() => expect(present(view, testId)).toBe(false), { timeout: 5_000 });
}

/** Open an agent's drawer in the roster. */
async function selectAgent(view: View, name: string): Promise<void> {
  const row = await find(view, `roster-row:${name}`);
  fireEvent.click(row.querySelector(".cc-entity__hit") as HTMLElement);
  await find(view, `roster-detail:${name}`);
}

function setField(scope: HTMLElement, testId: string, value: string): void {
  fireEvent.change(within(scope).getByTestId(testId), { target: { value } });
}

/** Show the roster's archived agents: they are read when the person opens
 * them. An open list stays open. */
async function openArchived(view: View): Promise<void> {
  const open = await find(view, "roster-archived-open");
  if (open.getAttribute("aria-expanded") !== "true") fireEvent.click(open);
}

/** Open New agent and wait for the team's profiles. */
async function openNewAgent(view: View): Promise<HTMLElement> {
  fireEvent.click(await find(view, "roster-new-agent"));
  const form = await find(view, "identity-new-agent");
  await within(form).findByTestId("identity-field:profile", undefined, { timeout: 5_000 });
  return form;
}

function calls(family: Family, method: string): Array<Record<string, unknown>> {
  return family.calls.filter((call: { method: string }) => call.method === method).map((call: { params: Record<string, unknown> }) => call.params);
}

/** The notice's own words: its headline and lines, without the collapsed
 * technical details. */
function noticeWords(notice: HTMLElement): string {
  return Array.from(notice.querySelectorAll(".identity-notice__title, .identity-notice__line"))
    .map((node) => node.textContent || "")
    .join(" ");
}

/** The gateway's own sentence of the refusal shown in `scope`, as its
 * Details disclosure holds it: collapsed, so no primary text. */
function detailsOf(scope: Element | null): string {
  const details = scope?.querySelector("details[data-testid='identity-refusal-details']");
  expect(details).not.toBeNull();
  expect((details as HTMLDetailsElement).open).toBe(false);
  expect(details?.querySelector("summary")?.textContent).toBe("Details");
  return details?.querySelector(".identity-refusal__gateway")?.textContent || "";
}

describe("long-term agents in the console", () => {
  beforeEach(() => {
    vi.stubGlobal("localStorage", window.localStorage);
    window.localStorage.clear();
  });
  afterEach(async () => {
    await settle(20);
    cleanup();
    vi.unstubAllGlobals();
    window.localStorage.clear();
  });

  it("a_person_creates_edits_archives_and_removes_an_agent_in_the_console", async () => {
    const family = createScriptedIdentityFamily();
    const { view } = await openRoster(family);

    // New agent: a profile of the directory, a name, and what it starts with.
    const form = await openNewAgent(view);
    fireEvent.click(within(form).getByTestId("identity-profile:worker"));
    setField(form, "identity-field:name", "scout");
    setField(form, "identity-field:display_name", "Scout");
    setField(form, "identity-field:instructions:0", "Scout the archive.");
    fireEvent.click(within(form).getByTestId("identity-peer:warden"));
    setField(form, "identity-field:first_message", "Introduce yourself.");
    fireEvent.submit(form);
    let notice = await find(view, "identity-notice");
    expect(noticeWords(notice)).toContain("Scout is created");
    const created = calls(family, "meta/identity/create");
    expect(created).toHaveLength(1);
    expect(created[0]).toEqual({
      request_id: expect.stringMatching(/\S/),
      agents: [{
        name: "scout",
        profile: "worker",
        display_name: "Scout",
        instructions: ["Scout the archive."],
        first_message: "Introduce yourself.",
        peers: ["warden"],
      }],
    });
    // The roster lists the new agent like every other.
    expect((await find(view, "roster-row:scout")).textContent).toContain("Scout");
    fireEvent.click(view.getByTestId("identity-cancel"));
    await gone(view, "identity-new-agent");

    // Edit: against the record read when the form opened, only what changed.
    await selectAgent(view, "scout");
    fireEvent.click(view.getByTestId("roster-action:scout:edit"));
    const edit = await find(view, "identity-edit:scout");
    await within(edit).findByTestId("identity-field:display_name", undefined, { timeout: 5_000 });
    const digest = family.handle("meta/identity/get", { name: "scout" }).result.spec_digest;
    setField(edit, "identity-field:display_name", "Chief scout");
    setField(edit, "identity-field:instructions:0", "Scout the vault.");
    fireEvent.submit(edit);
    notice = await find(view, "identity-notice");
    expect(noticeWords(notice)).toContain("is updated");
    expect(noticeWords(notice)).toContain("reach the agent at its next run");
    expect(calls(family, "meta/identity/update")).toEqual([{
      request_id: expect.stringMatching(/\S/),
      name: "scout",
      expected_spec_digest: digest,
      set: { display_name: "Chief scout", instructions: ["Scout the vault."] },
    }]);
    await waitFor(() => expect(view.getByTestId("roster-row:scout").textContent).toContain("Chief scout"), { timeout: 5_000 });
    fireEvent.click(view.getByTestId("identity-cancel"));

    // Archive: it leaves the roster and is listed as archived.
    await selectAgent(view, "scout");
    fireEvent.click(view.getByTestId("roster-action:scout:archive"));
    let confirm = await find(view, "identity-confirm:scout:archive");
    expect(confirm.textContent).toContain("Archive Chief scout?");
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    notice = await find(view, "identity-notice");
    expect(noticeWords(notice)).toContain("Chief scout is archived");
    await gone(view, "roster-row:scout");
    await openArchived(view);
    expect((await find(view, "roster-archived:scout")).textContent).toContain("Chief scout");
    fireEvent.click(view.getByTestId("identity-confirm-cancel"));

    // Unarchive: it is back in the roster.
    fireEvent.click(view.getByTestId("roster-archived-action:scout:unarchive"));
    confirm = await find(view, "identity-confirm:scout:unarchive");
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    notice = await find(view, "identity-notice");
    expect(noticeWords(notice)).toContain("Chief scout is back");
    await find(view, "roster-row:scout");
    await gone(view, "roster-archived:scout");
    fireEvent.click(view.getByTestId("identity-confirm-cancel"));

    // Remove: the confirmation says it is for good and names archive as
    // the reversible choice.
    await selectAgent(view, "scout");
    fireEvent.click(view.getByTestId("roster-action:scout:remove"));
    confirm = await find(view, "identity-confirm:scout:remove");
    expect(confirm.textContent).toContain("Its name can never be used again");
    expect(confirm.textContent).toContain("Archive is the reversible choice");
    expect(within(confirm).queryByTestId("identity-confirm-archive-instead")).not.toBeNull();
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    notice = await find(view, "identity-notice");
    expect(noticeWords(notice)).toContain("Chief scout is removed");
    expect(noticeWords(notice)).toContain("can never be used again");
    await gone(view, "roster-row:scout");
    expect(family.agents.has("scout")).toBe(false);

    // Each action was one request under its own id.
    const ids = ["create", "update", "archive", "unarchive", "remove"].flatMap((change) =>
      calls(family, `meta/identity/${change}`).map((params) => params.request_id));
    expect(ids).toHaveLength(5);
    expect(new Set(ids).size).toBe(5);
    view.unmount();
  }, 60_000);

  it("no_origin_badge_and_no_id_as_primary_text", async () => {
    // One agent the host configured (its record names nobody who asked), one
    // an agent asked for, one a person asked for.
    const family = createScriptedIdentityFamily();
    const key = 'principal:["human","example","dana"]';
    family.seed({
      name: "courier", display_name: "The courier", profile: "worker",
      provenance: { entered: "adopted", requester_kind: "human", requester: key, at_ms: 1_762_000_000_000, consent: { form: "decided" } },
    });
    // Two agents without a display name, one of them archived.
    family.seed({ name: "night-desk", profile: "worker" });
    family.seed({ name: "old-desk", profile: "worker", archived: true });
    const { view } = await openRoster(family);
    await find(view, "roster-row:warden");

    // The roster shows every agent the same way: the same parts, the same
    // marks, whoever asked for it.
    const shape = (node: Element): string => {
      const attributes = Array.from(node.attributes)
        .map((attribute) => attribute.name)
        .filter((name) => name.startsWith("data-") || name === "class" || name === "role")
        .sort()
        .map((name) => (name === "class" ? `class=${node.getAttribute("class")}` : name));
      return `${node.tagName}[${attributes.join(",")}](${Array.from(node.children).map(shape).join("")})`;
    };
    const rows = ["helper", "warden", "courier"].map((name) => view.getByTestId(`roster-row:${name}`));
    expect(shape(rows[1])).toBe(shape(rows[0]));
    expect(shape(rows[2])).toBe(shape(rows[0]));
    const roster = view.getByTestId("roster-panel");
    const origin = /\b(created|configured|declared|adopted|imported|store.owned|origin|custody|provenance|entered)\b/i;
    expect(roster.textContent || "").not.toMatch(origin);
    for (const node of Array.from(roster.querySelectorAll("*"))) {
      for (const attribute of Array.from(node.attributes)) {
        expect(`${attribute.name}=${attribute.value}`).not.toMatch(origin);
      }
    }
    // The name people read is the display name; the id is a tooltip.
    expect(rows[1].querySelector(".cc-entity__title")?.textContent).toBe("The warden");

    // Each agent's drawer offers the same controls.
    const controls: string[][] = [];
    for (const name of ["helper", "warden", "courier"]) {
      await selectAgent(view, name);
      controls.push(Array.from(view.getByTestId(`roster-detail:${name}`).querySelectorAll("[data-testid]"))
        .map((node) => (node.getAttribute("data-testid") || "").split(name).join("<agent>")));
    }
    expect(controls[0]).toContain("roster-action:<agent>:edit");
    expect(controls[1]).toEqual(controls[0]);
    expect(controls[2]).toEqual(controls[0]);

    // A change's outcome names the agent as people know it; ids stay in
    // Technical details.
    const form = await openNewAgent(view);
    // An agent without a display name reads by its name in words, in the
    // roster, among the peers and among the archived agents alike; the name
    // itself is a tooltip.
    expect(view.getByTestId("roster-row:night-desk").querySelector(".cc-entity__title")?.textContent).toBe("Night desk");
    const peer = within(form).getByTestId("identity-peer:night-desk").closest("label");
    expect(peer?.querySelector(".identity-form__option-name")?.textContent).toBe("Night desk");
    expect(peer?.querySelector("[title='night-desk']")).not.toBeNull();
    await openArchived(view);
    expect((await find(view, "roster-archived:old-desk")).querySelector(".identity-archived__name")?.textContent).toBe("Old desk");
    fireEvent.click(within(form).getByTestId("identity-profile:worker"));
    setField(form, "identity-field:name", "scout");
    setField(form, "identity-field:display_name", "Scout");
    fireEvent.submit(form);
    const notice = await find(view, "identity-notice");
    const operationId = family.handle("meta/identity/operation", { request_id: calls(family, "meta/identity/create")[0].request_id }).result.operation_id;
    expect(notice.querySelector(".identity-notice__title")?.textContent).toBe("Scout is created");
    expect(noticeWords(notice)).not.toContain(operationId);
    expect(noticeWords(notice)).not.toMatch(/sha256:|\bcommitted\b|\bseated\b/);
    const idNodes = Array.from(notice.querySelectorAll("*")).filter((node) =>
      Array.from(node.childNodes).some((child) => child.nodeType === 3 && (child.textContent || "").includes(operationId)));
    expect(idNodes.length).toBeGreaterThan(0);
    for (const node of idNodes) expect(node.closest(".cc-tech")).not.toBeNull();
    view.unmount();
    cleanup();

    // The details pane: who asked and when, as a record.
    const record = async (name: string): Promise<HTMLElement> => {
      window.localStorage.clear();
      seedDock({ id: `inspect:${name}`, kind: "identity-inspect", title: `${name} details`, identity: name, memberId: name });
      const details = render(<ConsoleApp baseUrl="" transport={transport(family)} />);
      await details.findByTestId(`inspect-panel:${name}`, undefined, { timeout: 5_000 });
      return details.findByTestId(`inspect-record:${name}`, undefined, { timeout: 5_000 });
    };
    const warden = await record("warden");
    const facts = (section: HTMLElement) => section.querySelector(".cc-kv")?.textContent || "";
    expect(facts(warden)).toContain("Asked for by");
    // An agent is named as the roster names it.
    expect(facts(warden)).toContain("Helper");
    expect(facts(warden)).toContain("Allowed by the host's rules");
    expect(warden.textContent || "").not.toMatch(origin);
    expect(facts(warden)).not.toMatch(/sha256:/);
    cleanup();

    // A person is named as a decision's requester is. The person's key is
    // shown nowhere: not as text, and not behind a tooltip or any other
    // attribute.
    const courier = await record("courier");
    expect(facts(courier)).toContain("dana");
    expect(facts(courier)).toContain("Decided by a person");
    expect(courier.textContent || "").not.toContain(key);
    for (const node of [courier, ...Array.from(courier.querySelectorAll("*"))]) {
      for (const attribute of Array.from(node.attributes)) {
        expect(`${attribute.name}=${attribute.value}`).not.toContain("principal:");
      }
    }
    expect(courier.textContent || "").not.toMatch(origin);
    cleanup();

    // An agent whose record names nobody shows no line of who asked.
    const helper = await record("helper");
    expect(facts(helper)).not.toContain("Asked for by");
    expect(helper.textContent || "").not.toMatch(origin);
  }, 60_000);

  it("buttons_follow_the_backend_flags", async () => {
    const CONTROLS = {
      new: "roster-new-agent",
      edit: "roster-action:warden:edit",
      archive: "roster-action:warden:archive",
      remove: "roster-action:warden:remove",
      unarchive: "roster-archived-action:oldhand:unarchive",
      archivedRemove: "roster-archived-action:oldhand:remove",
    } as const;
    type Control = keyof typeof CONTROLS;
    const all = Object.keys(CONTROLS) as Control[];
    const cases: Array<{ off: string; hidden: Control[]; apply: (family: Family) => void }> = [
      { off: "nothing", hidden: [], apply: () => {} },
      { off: "identity_changes.available", hidden: all, apply: (family) => { family.flags.available = false; } },
      { off: "identity_changes.can_create", hidden: ["new"], apply: (family) => { family.flags.can_create = false; } },
      { off: "identity_changes.can_change", hidden: ["edit", "archive", "unarchive"], apply: (family) => { family.flags.can_change = false; } },
      { off: "identity_changes.can_remove", hidden: ["remove", "archivedRemove"], apply: (family) => { family.flags.can_remove = false; } },
      { off: "affordances.can_change_identity", hidden: ["edit", "archive"], apply: (family) => family.setAffordances("warden", { can_change_identity: false }) },
      { off: "affordances.can_remove_identity", hidden: ["remove"], apply: (family) => family.setAffordances("warden", { can_remove_identity: false }) },
    ];
    for (const entry of cases) {
      window.localStorage.clear();
      const family = createScriptedIdentityFamily();
      family.seed({ name: "oldhand", display_name: "Old hand", archived: true });
      entry.apply(family);
      const { view } = await openRoster(family);
      await selectAgent(view, "warden");
      await openArchived(view);
      await find(view, "roster-archived:oldhand");
      const shown = all.filter((control) => present(view, CONTROLS[control]));
      expect({ off: entry.off, shown }).toEqual({ off: entry.off, shown: all.filter((control) => !entry.hidden.includes(control)) });
      // Where no change is offered the archived agents are still listed:
      // the console reads, and sends no change.
      if (entry.off === "identity_changes.available") {
        expect(view.getByTestId("roster-archived:oldhand").textContent).toContain("Old hand");
        expect(family.calls.map((call: { method: string }) => call.method).every((method: string) =>
          method === "meta/identity/list" || method === "meta/identity/get" || method === "meta/identity/operation")).toBe(true);
      }
      // Remove offers "Archive instead" only to a viewer who may archive
      // the agent.
      if (entry.off === "nothing" || entry.off === "affordances.can_change_identity") {
        fireEvent.click(view.getByTestId("roster-action:warden:remove"));
        const confirm = await find(view, "identity-confirm:warden:remove");
        expect(within(confirm).queryByTestId("identity-confirm-archive-instead") !== null).toBe(entry.off === "nothing");
      }
      view.unmount();
      cleanup();
    }

    // A read-only console offers no change, whatever the section says; the
    // archived agents are still listed.
    window.localStorage.clear();
    const fixed = createScriptedIdentityFamily();
    fixed.seed({ name: "oldhand", display_name: "Old hand", archived: true });
    const readOnly = await openRoster(fixed, { readOnly: true });
    await selectAgent(readOnly.view, "warden");
    await openArchived(readOnly.view);
    await find(readOnly.view, "roster-archived:oldhand");
    expect(all.filter((control) => present(readOnly.view, CONTROLS[control]))).toEqual([]);
    readOnly.view.unmount();
    cleanup();

    // A gateway that serves no method of the family: the console reads
    // nothing of it and shows nothing of it, whatever the section says.
    window.localStorage.clear();
    const unserved = createScriptedIdentityFamily();
    unserved.seed({ name: "oldhand", display_name: "Old hand", archived: true });
    unserved.flags.available = false;
    const { view } = await openRoster(unserved, { serveFamily: false });
    await selectAgent(view, "warden");
    await settle(300);
    expect(all.filter((control) => present(view, CONTROLS[control]))).toEqual([]);
    expect(present(view, "roster-archived")).toBe(false);
    expect(present(view, "roster-archived-open")).toBe(false);
    expect(unserved.calls).toEqual([]);
    view.unmount();
  }, 120_000);

  it("a_refusal_is_shown_at_its_field_with_its_sentence", async () => {
    const family = createScriptedIdentityFamily();
    const { view } = await openRoster(family);
    const form = await openNewAgent(view);
    fireEvent.click(within(form).getByTestId("identity-profile:worker"));

    // A name that is taken: the console's sentence, at the name, and the
    // gateway's own behind Details.
    setField(form, "identity-field:name", "warden");
    fireEvent.submit(form);
    const atName = await find(view, "identity-field-error:name");
    expect(atName.textContent).toBe("This name is taken. Choose another; a removed agent's name is never free again.");
    expect(detailsOf(form.querySelector("[data-field='name']")))
      .toBe("The name is taken: choose another. A removed agent's name is never available again.");
    expect(form.querySelector("[data-field='name']")?.getAttribute("data-invalid")).toBe("true");
    expect(within(form).getByTestId("identity-field:name").getAttribute("aria-invalid")).toBe("true");
    expect(form.textContent).not.toContain("name_taken");
    expect(present(view, "identity-notice")).toBe(false);
    // Changing the field takes its refusal away.
    setField(form, "identity-field:name", "scout");
    expect(present(view, "identity-field-error:name")).toBe(false);

    // A refusal that names the first message shows there.
    family.scriptNext("meta/identity/create", {
      refuse: {
        kind: "unsupported",
        field: "agents[0].first_message",
        feature: "a first message on a governed runtime",
        detail: "A governed runtime does not start an agent with a first message: leave it out.",
      },
    });
    setField(form, "identity-field:first_message", "Hello.");
    fireEvent.submit(form);
    expect((await find(view, "identity-field-error:first_message")).textContent)
      .toBe("This change needs something this version does not support. Leave it out.");
    expect(detailsOf(form.querySelector("[data-field='first_message']")))
      .toBe("A governed runtime does not start an agent with a first message: leave it out.");
    expect(form.textContent).not.toContain("unsupported");

    // A peer the gateway refuses (it is gone, or none this viewer may
    // name: the gateway says the same of both) shows at the peers.
    family.scriptNext("meta/identity/create", { refuse: { kind: "peer_unknown", field: "agents[0].peers[0]" } });
    setField(form, "identity-field:first_message", "");
    fireEvent.click(within(form).getByTestId("identity-peer:warden"));
    fireEvent.submit(form);
    expect((await find(view, "identity-field-error:peers")).textContent)
      .toBe("This peer is not an agent of this team that you can name. Pick another agent.");
    expect(detailsOf(form.querySelector("[data-field='peers']")))
      .toBe("The peer is no agent you may name: list agents of the mob, or other agents of the same request.");
    fireEvent.click(within(form).getByTestId("identity-peer:warden"));
    expect(present(view, "identity-field-error:peers")).toBe(false);

    // A refusal of the request as a whole is the form's own notice.
    family.scriptNext("meta/identity/create", { refuse: { kind: "change_denied", change: "create" } });
    setField(form, "identity-field:first_message", "");
    fireEvent.submit(form);
    const whole = await find(view, "identity-notice");
    expect(whole.querySelector("[data-outcome='refused']")?.textContent)
      .toBe("The host's rules do not allow you this change. It was not made, and asking again changes nothing.");
    expect(detailsOf(whole)).toBe("The host's rules deny this change for you: it was not made, and repeating it changes nothing.");
    expect(whole.textContent).not.toContain("change_denied");
    expect(present(view, "identity-retry")).toBe(false);
    expect(family.agents.has("scout")).toBe(false);
    fireEvent.click(view.getByTestId("identity-cancel"));

    // An agent the host's configuration declares: the gateway refuses the
    // change, and the console says so in its own words. What the gateway
    // says of other ways is behind Details.
    await selectAgent(view, "helper");
    fireEvent.click(view.getByTestId("roster-action:helper:edit"));
    const edit = await find(view, "identity-edit:helper");
    await within(edit).findByTestId("identity-field:display_name", undefined, { timeout: 5_000 });
    setField(edit, "identity-field:display_name", "Chief helper");
    fireEvent.submit(edit);
    const refused = await find(view, "identity-notice");
    expect(refused.querySelector("[data-outcome='refused']")?.textContent)
      .toBe("This agent is set up in the host's configuration and cannot be changed here. Change it in the configuration.");
    expect(noticeWords(refused)).not.toContain("adopt");
    expect(detailsOf(refused)).toContain("declared by the host's configuration");
    expect(refused.textContent).not.toContain("owner_read_only");

    // A refusal that names a field of the update shows at that field.
    fireEvent.click(view.getByTestId("identity-cancel"));
    await selectAgent(view, "warden");
    fireEvent.click(view.getByTestId("roster-action:warden:edit"));
    const wardenEdit = await find(view, "identity-edit:warden");
    await within(wardenEdit).findByTestId("identity-field:display_name", undefined, { timeout: 5_000 });
    family.scriptNext("meta/identity/update", { refuse: { kind: "profile_unknown", field: "set.profile", profile: "reviewer" } });
    fireEvent.click(within(wardenEdit).getByTestId("identity-profile:reviewer"));
    fireEvent.submit(wardenEdit);
    expect((await find(view, "identity-field-error:profile")).textContent)
      .toBe("This team has no such profile. Pick one of the team's profiles.");
    expect(detailsOf(wardenEdit.querySelector("[data-field='profile']")))
      .toBe("The mob has no profile of this name: name one of the mob's profiles.");
    fireEvent.click(view.getByTestId("identity-cancel"));

    // An agent that is gone when its change is confirmed: the read is
    // refused, its sentence shows, and no change is sent.
    await selectAgent(view, "warden");
    fireEvent.click(view.getByTestId("roster-action:warden:archive"));
    const confirm = await find(view, "identity-confirm:warden:archive");
    family.agents.delete("warden");
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    const unread = await find(view, "identity-notice");
    expect(unread.querySelector("[data-outcome='unread']")?.textContent).toBe("This team has no agent of this name. It may have been removed.");
    expect(detailsOf(unread)).toBe("No agent has this name: list the agents and name one.");
    expect(unread.getAttribute("role")).toBe("alert");
    expect(calls(family, "meta/identity/archive")).toEqual([]);
    view.unmount();
  }, 60_000);

  it("a_profile_change_is_judged_by_the_gateway_not_by_what_a_create_may_name", async () => {
    // The directory's verdicts say what a new agent may be created with:
    // here a reviewer needs a decision and an operator is not allowed. A
    // change of an existing agent's profile is another change under its
    // own rules, so the Edit form shows none of them and asks the gateway.
    const family = createScriptedIdentityFamily();
    const { view } = await openRoster(family);
    await selectAgent(view, "warden");
    fireEvent.click(view.getByTestId("roster-action:warden:edit"));
    const edit = await find(view, "identity-edit:warden");
    const profiles = await within(edit).findByTestId("identity-field:profile", undefined, { timeout: 5_000 });
    const digest = family.handle("meta/identity/get", { name: "warden" }).result.spec_digest;
    for (const profile of ["worker", "reviewer", "operator"]) {
      expect((within(edit).getByTestId(`identity-profile:${profile}`) as HTMLInputElement).disabled).toBe(false);
    }
    expect(profiles.textContent).not.toContain("Not allowed");
    expect(profiles.textContent).not.toContain("Needs a decision");
    expect(profiles.textContent).not.toContain("A person decides each new reviewer.");
    expect(profiles.textContent).not.toContain("keep operators to its configuration");
    // The profile the agent has asks nothing.
    expect(present(view, "identity-profile-plan")).toBe(false);
    expect(calls(family, "meta/identity/plan")).toEqual([]);

    // Another profile: the gateway is asked what this change of this agent
    // would get, and its answer shows at the field. It allows the profile a
    // create may not name.
    fireEvent.click(within(edit).getByTestId("identity-profile:operator"));
    let plan = await find(view, "identity-profile-plan");
    await waitFor(() => expect(view.getByTestId("identity-profile-plan").getAttribute("data-plan")).toBe("allow"), { timeout: 5_000 });
    plan = view.getByTestId("identity-profile-plan");
    expect(plan.textContent).toContain("The host's rules allow this change");
    expect(calls(family, "meta/identity/plan")).toEqual([{
      change: "update",
      request_id: expect.stringMatching(/\S/),
      name: "warden",
      expected_spec_digest: digest,
      set: { profile: "operator" },
    }]);

    // A change the rules hold for a person: said before it is sent.
    family.setProfileChange("reviewer", "require_approval");
    fireEvent.click(within(edit).getByTestId("identity-profile:reviewer"));
    await waitFor(() => expect(view.getByTestId("identity-profile-plan").getAttribute("data-plan")).toBe("require_approval"), { timeout: 5_000 });
    expect(view.getByTestId("identity-profile-plan").textContent).toContain("A person must decide this change before it is made");

    // A change the rules deny: the refusal shows at the profile, as a
    // sentence, and the profile stays one the person can pick.
    family.setProfileChange("operator", "deny");
    fireEvent.click(within(edit).getByTestId("identity-profile:operator"));
    const refused = await find(view, "identity-field-error:profile");
    expect(refused.textContent).toBe("The host's rules do not allow you this change. It was not made, and asking again changes nothing.");
    expect(detailsOf(edit.querySelector("[data-field='profile']")))
      .toBe("The host's rules deny this change for you: it was not made, and repeating it changes nothing.");
    expect(present(view, "identity-profile-plan")).toBe(false);
    expect((within(edit).getByTestId("identity-profile:operator") as HTMLInputElement).disabled).toBe(false);

    // Back on the agent's own profile nothing is asked and nothing is said.
    fireEvent.click(within(edit).getByTestId("identity-profile:worker"));
    await gone(view, "identity-field-error:profile");
    expect(present(view, "identity-profile-plan")).toBe(false);
    expect(calls(family, "meta/identity/plan")).toHaveLength(3);

    // A plan writes nothing and holds nothing: the change is the Save, a
    // request of its own, which the gateway holds as it said it would.
    fireEvent.click(within(edit).getByTestId("identity-profile:reviewer"));
    await waitFor(() => expect(view.getByTestId("identity-profile-plan").getAttribute("data-plan")).toBe("require_approval"), { timeout: 5_000 });
    expect(family.agents.get("warden").profile).toBe("worker");
    expect(family.decisionStatuses()).toEqual([]);
    fireEvent.submit(edit);
    const notice = await find(view, "identity-notice");
    expect(notice.querySelector("[data-outcome='pending_decision']")).not.toBeNull();
    const updates = calls(family, "meta/identity/update");
    expect(updates).toEqual([{
      request_id: expect.stringMatching(/\S/),
      name: "warden",
      expected_spec_digest: digest,
      set: { profile: "reviewer" },
    }]);
    expect(calls(family, "meta/identity/plan").map((params) => params.request_id)).not.toContain(updates[0].request_id);
    view.unmount();
  }, 60_000);

  it("an_agent_that_changed_under_a_form_is_read_again_and_nothing_is_sent_on_its_own", async () => {
    const family = createScriptedIdentityFamily();
    const { view, harness } = await openRoster(family);
    const reads = () => calls(family, "meta/identity/get").filter((params) => params.name === "warden").length;
    const digestNow = () => family.handle("meta/identity/get", { name: "warden" }).result.spec_digest as string;
    await selectAgent(view, "warden");
    fireEvent.click(view.getByTestId("roster-action:warden:edit"));
    const edit = await find(view, "identity-edit:warden");
    await within(edit).findByTestId("identity-field:display_name", undefined, { timeout: 5_000 });
    const opened = digestNow();

    // Somebody else changes the agent while the form is open.
    family.agents.get("warden").instructions = ["Guard the vault."];
    const moved = digestNow();
    expect(moved).not.toBe(opened);
    setField(edit, "identity-field:display_name", "Chief warden");
    fireEvent.submit(edit);
    let notice = await find(view, "identity-notice");
    // The refusal is shown with Read it again, and nothing else: no Try
    // again, and no second request with the digest the refusal names.
    expect(notice.querySelector("[data-outcome='refused']")?.textContent)
      .toBe("This agent changed since you opened it. Read it again, then make your change.");
    expect(detailsOf(notice)).toBe("The agent changed since you read it: read it again and send the current spec digest.");
    expect(notice.textContent).not.toContain("stale_spec");
    expect(notice.textContent).not.toContain(moved);
    expect(within(notice).queryByTestId("identity-retry")).toBeNull();
    await settle(50);
    let updates = calls(family, "meta/identity/update");
    expect(updates).toHaveLength(1);
    expect(updates[0].expected_spec_digest).toBe(opened);
    expect(family.agents.get("warden").display_name).toBe("The warden");

    // Read it again reads the record: the form shows what the agent is now.
    const before = reads();
    fireEvent.click(within(notice).getByTestId("identity-reload"));
    await waitFor(() => expect(reads()).toBe(before + 1), { timeout: 5_000 });
    await waitFor(() => expect((within(edit).getByTestId("identity-field:instructions:0") as HTMLTextAreaElement).value).toBe("Guard the vault."), { timeout: 5_000 });
    expect(present(view, "identity-notice")).toBe(false);
    expect(calls(family, "meta/identity/update")).toHaveLength(1);
    // The next Save is another request: the record's digest, a new id.
    setField(edit, "identity-field:display_name", "Chief warden");
    fireEvent.submit(edit);
    await waitFor(() => expect(noticeWords(view.getByTestId("identity-notice"))).toContain("is updated"), { timeout: 5_000 });
    updates = calls(family, "meta/identity/update");
    expect(updates).toHaveLength(2);
    expect(updates[1].expected_spec_digest).toBe(moved);
    expect(updates[1].request_id).not.toBe(updates[0].request_id);
    fireEvent.click(view.getByTestId("identity-cancel"));

    // A confirmation: the agent changes between its read and its change.
    await selectAgent(view, "warden");
    fireEvent.click(view.getByTestId("roster-action:warden:archive"));
    const confirm = await find(view, "identity-confirm:warden:archive");
    const read = digestNow();
    const execute = harness.executeCommand!;
    let changed = false;
    harness.executeCommand = async (input) => {
      if (!changed && consoleCommandMethod(input.command) === "meta/identity/archive") {
        changed = true;
        family.agents.get("warden").labels = { team: "vault" };
      }
      return execute(input);
    };
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    notice = await find(view, "identity-notice");
    expect(notice.querySelector("[data-outcome='refused']")).not.toBeNull();
    expect(within(notice).queryByTestId("identity-retry")).toBeNull();
    await settle(50);
    let archives = calls(family, "meta/identity/archive");
    expect(archives).toHaveLength(1);
    expect(archives[0].expected_spec_digest).toBe(read);
    expect(family.agents.get("warden").archived).toBe(false);
    // Read it again brings the question back; confirming reads the record
    // and sends another request.
    const readsBefore = reads();
    fireEvent.click(within(notice).getByTestId("identity-reload"));
    await gone(view, "identity-notice");
    expect(confirm.textContent).toContain("Archive Chief warden?");
    expect(calls(family, "meta/identity/archive")).toHaveLength(1);
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    await waitFor(() => expect(noticeWords(view.getByTestId("identity-notice"))).toContain("Chief warden is archived"), { timeout: 5_000 });
    expect(reads()).toBe(readsBefore + 1);
    archives = calls(family, "meta/identity/archive");
    expect(archives).toHaveLength(2);
    expect(archives[1].expected_spec_digest).not.toBe(read);
    expect(archives[1].request_id).not.toBe(archives[0].request_id);
    view.unmount();
  }, 60_000);

  it("a_peer_the_directory_does_not_allow_cannot_be_added_and_one_the_agent_has_can_be_taken_away", async () => {
    const family = createScriptedIdentityFamily();
    family.seed({ name: "courier", display_name: "The courier", profile: "worker" });
    family.setPeerVerdict("helper", { verdict: "not_allowed", reason: "The host's rules keep the helper to its own desk." });
    family.setPeerVerdict("courier", { verdict: "needs_decision", lines: ["A person decides each link to the courier."] });
    const { view } = await openRoster(family);
    const form = await openNewAgent(view);
    const peers = within(form).getByTestId("identity-field:peers");
    // A peer the rules do not allow is shown with the rule's reason and
    // cannot be picked; one that needs a decision can, with its line; an
    // allowed one carries no mark.
    expect((within(form).getByTestId("identity-peer:helper") as HTMLInputElement).disabled).toBe(true);
    expect(peers.textContent).toContain("Not allowed");
    expect(peers.textContent).toContain("The host's rules keep the helper to its own desk.");
    expect((within(form).getByTestId("identity-peer:courier") as HTMLInputElement).disabled).toBe(false);
    expect(peers.textContent).toContain("Needs a decision");
    expect(peers.textContent).toContain("A person decides each link to the courier.");
    expect((within(form).getByTestId("identity-peer:warden") as HTMLInputElement).disabled).toBe(false);
    expect(peers.textContent).not.toContain("Allowed");
    fireEvent.click(view.getByTestId("identity-cancel"));

    // The warden has the helper as a peer already: that link can be taken
    // away, and once it is, it cannot be added back.
    await selectAgent(view, "warden");
    fireEvent.click(view.getByTestId("roster-action:warden:edit"));
    const edit = await find(view, "identity-edit:warden");
    const helper = await within(edit).findByTestId("identity-peer:helper", undefined, { timeout: 5_000 }) as HTMLInputElement;
    expect(helper.checked).toBe(true);
    expect(helper.disabled).toBe(false);
    fireEvent.click(helper);
    expect(helper.checked).toBe(false);
    expect(helper.disabled).toBe(true);
    fireEvent.submit(edit);
    await waitFor(() => expect(noticeWords(view.getByTestId("identity-notice"))).toContain("is updated"), { timeout: 5_000 });
    expect(calls(family, "meta/identity/update")).toEqual([{
      request_id: expect.stringMatching(/\S/),
      name: "warden",
      expected_spec_digest: expect.stringMatching(/^sha256:/),
      set: {},
      peers_remove: ["helper"],
    }]);
    view.unmount();
  }, 60_000);

  it("an_agents_details_offer_the_same_changes_and_read_its_record_again", async () => {
    const family = createScriptedIdentityFamily();
    seedDock({ id: "inspect:warden", kind: "identity-inspect", title: "The warden details", identity: "warden", memberId: "warden" });
    const view = render(<ConsoleApp baseUrl="" transport={transport(family)} />);
    await find(view, "inspect-panel:warden");
    const record = await find(view, "inspect-record:warden");
    // The digest is among the technical details, the revision with it.
    expect(record.querySelector(".cc-tech")?.textContent).toContain("Spec revision");

    // Edit, from the details' menu.
    fireEvent.click(view.getByTestId("inspect-menu:warden"));
    fireEvent.click(view.getByTestId("inspect-action:warden:edit"));
    const edit = await find(view, "identity-edit:warden");
    await within(edit).findByTestId("identity-field:instructions:1", undefined, { timeout: 5_000 });
    // Each section of the instructions is its own text area.
    expect((within(edit).getByTestId("identity-field:instructions:0") as HTMLTextAreaElement).value).toBe("Guard the records.");
    expect((within(edit).getByTestId("identity-field:instructions:1") as HTMLTextAreaElement).value).toBe("Answer in one paragraph.");
    family.setInstructionOutcome("warden", { kind: "applies_at_next_session", reason: "slot_limit_exceeded" });
    setField(edit, "identity-field:instructions:1", "Answer in two paragraphs.");
    fireEvent.click(within(edit).getByTestId("identity-label-add"));
    setField(edit, "identity-label-key:1", "floor");
    setField(edit, "identity-label-value:1", "2");
    fireEvent.click(within(edit).getByTestId("identity-peer:helper"));
    fireEvent.submit(edit);
    const notice = await find(view, "identity-notice");
    expect(noticeWords(notice)).toContain("at its next new conversation: the team holds as many instruction records as it can.");
    expect(noticeWords(notice)).not.toContain("slot_limit_exceeded");
    expect(calls(family, "meta/identity/update")).toEqual([{
      request_id: expect.stringMatching(/\S/),
      name: "warden",
      expected_spec_digest: expect.stringMatching(/^sha256:/),
      set: { instructions: ["Guard the records.", "Answer in two paragraphs."], labels: { floor: "2" } },
      peers_remove: ["helper"],
    }]);
    // The record is read again after the change.
    await waitFor(() => expect(calls(family, "meta/identity/get").length).toBeGreaterThanOrEqual(3), { timeout: 5_000 });
    fireEvent.click(view.getByTestId("identity-cancel"));

    // Remove offers archive instead, and archive is its own request.
    fireEvent.click(view.getByTestId("inspect-menu:warden"));
    fireEvent.click(view.getByTestId("inspect-action:warden:remove"));
    const confirm = await find(view, "identity-confirm:warden:remove");
    fireEvent.click(within(confirm).getByTestId("identity-confirm-archive-instead"));
    const archive = await find(view, "identity-confirm:warden:archive");
    fireEvent.click(within(archive).getByTestId("identity-confirm-yes"));
    await waitFor(() => expect(noticeWords(view.getByTestId("identity-notice"))).toContain("The warden is archived"), { timeout: 5_000 });
    expect(calls(family, "meta/identity/remove")).toEqual([]);
    expect(calls(family, "meta/identity/archive")).toHaveLength(1);
    view.unmount();
    cleanup();

    // An agent the viewer may not change offers no change in its menu, and
    // neither does a console that offers no change at all; the record of
    // who asked is read either way.
    for (const apply of [
      (family: Family) => family.setAffordances("warden", { can_change_identity: false, can_remove_identity: false }),
      (family: Family) => { family.flags.available = false; },
    ]) {
      window.localStorage.clear();
      const refused = createScriptedIdentityFamily();
      apply(refused);
      seedDock({ id: "inspect:warden", kind: "identity-inspect", title: "The warden details", identity: "warden", memberId: "warden" });
      const closed = render(<ConsoleApp baseUrl="" transport={transport(refused)} />);
      expect((await find(closed, "inspect-record:warden")).textContent).toContain("Asked for by");
      fireEvent.click(closed.getByTestId("inspect-menu:warden"));
      for (const change of ["edit", "archive", "remove"]) {
        expect(present(closed, `inspect-action:warden:${change}`)).toBe(false);
      }
      closed.unmount();
      cleanup();
    }
  }, 60_000);

  it("the_requesters_own_decision_card_offers_withdraw_and_no_approve", async () => {
    const family = createScriptedIdentityFamily();
    const { view } = await openRoster(family);
    const form = await openNewAgent(view);
    fireEvent.click(within(form).getByTestId("identity-profile:reviewer"));
    setField(form, "identity-field:name", "auditor");
    setField(form, "identity-field:display_name", "The auditor");
    fireEvent.submit(form);
    await find(view, "identity-notice");
    const requestId = calls(family, "meta/identity/create")[0].request_id;

    // The approvals view lists the decision the request waits for. The
    // card is the requester's own: it says so, offers no approval, and
    // offers the withdrawal.
    fireEvent.click(await find(view, "nav:gating"));
    const card = await find(view, "decision-card:decision-1");
    expect(card.querySelector(".cc-approval__title")?.textContent).toBe("New long-term agent");
    expect(card.textContent).toContain("You asked for this change");
    expect(card.textContent).toContain("a person cannot approve their own request");
    // Its one control is the withdrawal (the technical details keep their copy buttons).
    expect(Array.from(card.querySelectorAll("button"))
      .filter((button) => button.closest(".cc-approval__details") === null)
      .map((button) => button.textContent))
      .toEqual(["Withdraw request"]);
    expect(card.textContent).not.toMatch(/\bapprove\b(?! their own request)/i);

    // A withdrawal the gateway refuses shows its sentence on the card.
    family.scriptNext("meta/identity/withdraw", {
      refuse: { kind: "decision_closed", state: "approved", detail: "The decision is closed: it was approved." },
    });
    fireEvent.click(within(card).getByTestId("decision-card-withdraw"));
    expect((await find(view, "decision-card-withdraw-refused")).textContent).toBe("The decision has ended. Ask again if the change is still wanted.");
    expect(detailsOf(card)).toBe("The decision is closed: it was approved.");

    // The withdrawal names the person's own request, and the card goes.
    fireEvent.click(within(card).getByTestId("decision-card-withdraw"));
    await gone(view, "decision-card:decision-1");
    expect(calls(family, "meta/identity/withdraw")).toEqual([{ request_id: requestId }, { request_id: requestId }]);
    expect(family.agents.has("auditor")).toBe(false);
    view.unmount();
  }, 60_000);

  it("the_archived_agents_are_read_when_the_person_opens_them", async () => {
    // The console reads the roster again on its own clock; that clock is
    // the row's to turn.
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      const family = createScriptedIdentityFamily();
      family.seed({ name: "oldhand", display_name: "Old hand", archived: true });
      family.seed({ name: "relic", display_name: "The relic", archived: true });
      const lists = () => calls(family, "meta/identity/list").length;
      const { view } = await openRoster(family);
      // The roster offers the archived agents behind a control, without a
      // count: it has read none of them.
      const open = await find(view, "roster-archived-open");
      expect(open.getAttribute("aria-expanded")).toBe("false");
      expect(open.textContent).toBe("Archived agents");
      expect(present(view, "roster-archived")).toBe(false);
      await selectAgent(view, "warden");
      await settle(100);
      expect(lists()).toBe(0);
      // Nor does the roster's own next read read them.
      await act(async () => {
        vi.advanceTimersByTime(15_000);
      });
      await settle(100);
      expect(lists()).toBe(0);

      // Opening them reads them, once.
      fireEvent.click(open);
      await find(view, "roster-archived:oldhand");
      await find(view, "roster-archived:relic");
      expect(open.getAttribute("aria-expanded")).toBe("true");
      expect(view.getByTestId("roster-archived").textContent).toContain("2");
      expect(lists()).toBe(1);

      // Somebody else unarchives an agent: the roster's next read lists it,
      // and the open list is not read again for that.
      const oldhand = family.agents.get("oldhand");
      oldhand.archived = false;
      oldhand.readiness = "dormant";
      await act(async () => {
        vi.advanceTimersByTime(15_000);
      });
      await find(view, "roster-row:oldhand");
      await settle(100);
      expect(lists()).toBe(1);
      // Closing the list and opening it again reads it again.
      fireEvent.click(open);
      await gone(view, "roster-archived");
      expect(lists()).toBe(1);
      fireEvent.click(open);
      await find(view, "roster-archived:relic");
      await gone(view, "roster-archived:oldhand");
      expect(lists()).toBe(2);

      // A change this page makes reads an open list again: the relic leaves
      // it when it is unarchived here.
      fireEvent.click(view.getByTestId("roster-archived-action:relic:unarchive"));
      fireEvent.click(within(await find(view, "identity-confirm:relic:unarchive")).getByTestId("identity-confirm-yes"));
      await find(view, "roster-row:relic");
      await gone(view, "roster-archived:relic");
      expect(lists()).toBe(3);
      // An open list without an archived agent says so.
      expect(view.getByTestId("roster-archived").textContent).toContain("No archived agents");
      view.unmount();
    } finally {
      vi.useRealTimers();
    }
  }, 60_000);

  it("a_confirmation_never_acts_on_another_agent_than_the_one_it_names", async () => {
    // The roster beside an agent's details: opening another agent's
    // details from the roster shows them in the same pane, in place.
    const family = createScriptedIdentityFamily();
    family.seed({ name: "courier", display_name: "The courier", profile: "worker" });
    window.localStorage.setItem(`meta-console-dock-state:${JSON.stringify(["", SCOPE])}`, JSON.stringify({
      tabs: [{
        id: "tab-1", presetId: "single",
        layout: { kind: "split", id: "split-1", direction: "horizontal", ratio: 0.5, first: { kind: "panel", panelId: "panel-1" }, second: { kind: "panel", panelId: "panel-2" } },
      }],
      panels: [
        { id: "panel-1", mode: "console", target: ROSTER },
        { id: "panel-2", mode: "console", target: { id: "inspect:warden", kind: "identity-inspect", title: "The warden details", identity: "warden", memberId: "warden" } },
      ],
      activeTabId: "tab-1",
      focusedPanelId: "panel-1",
    }));
    const view = render(<ConsoleApp baseUrl="" transport={transport(family)} />);
    await find(view, "inspect-record:warden");

    // Remove is opened for the warden.
    fireEvent.click(view.getByTestId("inspect-menu:warden"));
    fireEvent.click(view.getByTestId("inspect-action:warden:remove"));
    expect((await find(view, "identity-confirm:warden:remove")).textContent).toContain("Remove The warden for good?");

    // The pane is then shown the courier's details, from the roster.
    const roster = view.getByTestId("roster-panel");
    fireEvent.mouseDown(roster);
    fireEvent.click(view.getByTestId("roster-row:courier").querySelector(".cc-entity__hit") as HTMLElement);
    fireEvent.click(within(await find(view, "roster-detail:courier")).getByRole("button", { name: "Details" }));
    await find(view, "inspect-panel:courier");
    await gone(view, "inspect-panel:warden");

    // No confirmation is left that could act on the courier: neither the
    // warden's question over the courier's pane, nor any other.
    expect(view.container.querySelectorAll("[data-testid^='identity-confirm:']").length).toBe(0);
    expect(calls(family, "meta/identity/remove")).toEqual([]);
    expect(family.agents.has("courier")).toBe(true);
    expect(family.agents.has("warden")).toBe(true);
    view.unmount();
  }, 60_000);

  it("a_confirmation_takes_focus_and_is_left_by_escape_or_a_click_outside", async () => {
    const family = createScriptedIdentityFamily();
    seedDock({ id: "inspect:warden", kind: "identity-inspect", title: "The warden details", identity: "warden", memberId: "warden" });
    const harness = transport(family);
    // The archive's answer waits until the row lets it go.
    let release = () => {};
    const held = new Promise<void>((resolve) => { release = resolve; });
    const execute = harness.executeCommand!;
    harness.executeCommand = async (input) => {
      if (consoleCommandMethod(input.command) === "meta/identity/archive") await held;
      return execute(input);
    };
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await find(view, "inspect-record:warden");
    const open = async (change: string): Promise<HTMLElement> => {
      const menu = view.getByTestId("inspect-menu:warden");
      menu.focus();
      fireEvent.click(menu);
      fireEvent.click(view.getByTestId(`inspect-action:warden:${change}`));
      return find(view, `identity-confirm:warden:${change}`);
    };

    // The confirmation of a removal is a dialog of its own: named by its
    // question, described by what happens, with focus on the safe choice.
    let confirm = await open("remove");
    expect(confirm.getAttribute("role")).toBe("alertdialog");
    expect(confirm.getAttribute("aria-modal")).toBe("true");
    expect(document.getElementById(confirm.getAttribute("aria-labelledby") || "")?.textContent).toContain("Remove The warden for good?");
    expect(document.getElementById(confirm.getAttribute("aria-describedby") || "")?.textContent).toContain("can never be used again");
    const cancel = within(confirm).getByTestId("identity-confirm-cancel");
    expect(document.activeElement).toBe(cancel);
    // Tab stays inside it: from its last control to its first, and back.
    fireEvent.keyDown(cancel, { key: "Tab" });
    expect(document.activeElement).toBe(within(confirm).getByTestId("identity-confirm-yes"));
    fireEvent.keyDown(document.activeElement!, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(cancel);
    // Escape cancels, and focus goes back to the control that asked.
    fireEvent.keyDown(confirm, { key: "Escape" });
    await gone(view, "identity-confirm:warden:remove");
    expect(document.activeElement).toBe(view.getByTestId("inspect-menu:warden"));

    // So does a click anywhere outside it; a click inside it does not.
    confirm = await open("remove");
    fireEvent.pointerDown(within(confirm).getByTestId("identity-confirm-yes"));
    expect(present(view, "identity-confirm:warden:remove")).toBe(true);
    fireEvent.pointerDown(view.getByTestId("inspect-record:warden"));
    await gone(view, "identity-confirm:warden:remove");
    expect(calls(family, "meta/identity/remove")).toEqual([]);

    // While the change is being sent neither leaves it: its answer may be
    // that none came, and that must be seen.
    confirm = await open("archive");
    const yes = within(confirm).getByTestId("identity-confirm-yes") as HTMLButtonElement;
    fireEvent.click(yes);
    await waitFor(() => expect(yes.disabled).toBe(true), { timeout: 5_000 });
    fireEvent.keyDown(confirm, { key: "Escape" });
    fireEvent.pointerDown(view.getByTestId("inspect-record:warden"));
    expect(present(view, "identity-confirm:warden:archive")).toBe(true);
    await act(async () => { release(); });
    await waitFor(() => expect(noticeWords(view.getByTestId("identity-notice"))).toContain("The warden is archived"), { timeout: 5_000 });
    // The outcome keeps the dialog named, and focus is on its one control.
    confirm = view.getByTestId("identity-confirm:warden:archive");
    expect(confirm.getAttribute("aria-labelledby")).toBeNull();
    expect(confirm.getAttribute("aria-label")).toBe("Archive The warden");
    expect(document.activeElement).toBe(within(confirm).getByTestId("identity-confirm-cancel"));
    fireEvent.keyDown(confirm, { key: "Escape" });
    await gone(view, "identity-confirm:warden:archive");
    expect(calls(family, "meta/identity/archive")).toHaveLength(1);
    view.unmount();
    cleanup();

    // The roster's confirmation is the same dialog.
    window.localStorage.clear();
    const roster = await openRoster(createScriptedIdentityFamily());
    await selectAgent(roster.view, "warden");
    fireEvent.click(roster.view.getByTestId("roster-action:warden:remove"));
    const asked = await find(roster.view, "identity-confirm:warden:remove");
    await waitFor(() => expect(document.activeElement).toBe(within(asked).getByTestId("identity-confirm-cancel")), { timeout: 5_000 });
    fireEvent.keyDown(asked, { key: "Escape" });
    await gone(roster.view, "identity-confirm:warden:remove");
    roster.view.unmount();
  }, 60_000);

  it("an_answer_that_refuses_is_shown_as_a_refusal_never_as_no_answer", async () => {
    // An archived agent's Unarchive is offered by the section's flag; the
    // gateway judges the agent when the change is sent, and may say no.
    const family = createScriptedIdentityFamily();
    family.seed({ name: "oldhand", display_name: "Old hand", archived: true });
    const { view } = await openRoster(family);
    await openArchived(view);
    fireEvent.click(await find(view, "roster-archived-action:oldhand:unarchive"));
    const confirm = await find(view, "identity-confirm:oldhand:unarchive");
    family.scriptNext("meta/identity/unarchive", {
      error: { code: -32030, message: "access denied", data: { kind: "access_denied", action: "agent.identity.manage" } },
    });
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    const notice = await find(view, "identity-notice");
    // A definite refusal: nothing was made, nothing is offered again, and
    // the notice does not say the change may have been made.
    expect(notice.querySelector("[data-outcome='refused']")).not.toBeNull();
    expect(notice.getAttribute("role")).toBe("alert");
    expect(notice.textContent).not.toContain("may or may not");
    expect(notice.textContent).not.toContain("no answer");
    expect(within(notice).queryByTestId("identity-retry")).toBeNull();
    expect(notice.querySelector("[data-outcome='refused']")?.textContent).toBe("You do not have access to this. Nothing was changed.");
    expect(detailsOf(notice)).toBe("access denied");
    expect(family.agents.get("oldhand").archived).toBe(true);
    view.unmount();
  }, 60_000);

  it("a_pending_decision_is_shown_and_can_be_withdrawn", async () => {
    const family = createScriptedIdentityFamily({ now: () => Date.UTC(2030, 0, 1, 9, 0, 0) });
    const { view } = await openRoster(family);
    let form = await openNewAgent(view);
    // The directory says a reviewer needs a decision, with the decision's line.
    expect(within(form).getByTestId("identity-field:profile").textContent).toContain("Needs a decision");
    expect(within(form).getByTestId("identity-field:profile").textContent).toContain("A person decides each new reviewer.");
    // A profile the host's rules do not allow is shown with its reason and cannot be picked.
    expect((within(form).getByTestId("identity-profile:operator") as HTMLInputElement).disabled).toBe(true);
    expect(within(form).getByTestId("identity-field:profile").textContent).toContain("The host's rules keep operators to its configuration.");
    fireEvent.click(within(form).getByTestId("identity-profile:reviewer"));
    setField(form, "identity-field:name", "auditor");
    setField(form, "identity-field:display_name", "The auditor");
    fireEvent.submit(form);

    let notice = await find(view, "identity-notice");
    expect(notice.querySelector("[data-outcome='pending_decision']")?.textContent).toBe("A person must decide this change");
    expect(noticeWords(notice)).toContain("Nothing has changed yet");
    expect(noticeWords(notice)).toContain("you cannot approve your own request");
    // The deadline is the outcome's own.
    const deadline = within(notice).getByTestId("identity-deadline").textContent || "";
    expect(deadline).toContain("The decision is open until");
    expect(deadline).toMatch(/2030/);
    expect(family.agents.has("auditor")).toBe(false);
    expect(present(view, "roster-row:auditor")).toBe(false);

    // Withdraw names the person's own request.
    fireEvent.click(within(notice).getByTestId("identity-withdraw"));
    await waitFor(() => expect(view.getByTestId("identity-notice").textContent).toContain("The request is withdrawn"), { timeout: 5_000 });
    expect(calls(family, "meta/identity/withdraw")).toEqual([{ request_id: calls(family, "meta/identity/create")[0].request_id }]);
    expect(family.handle("meta/identity/operation", { request_id: calls(family, "meta/identity/create")[0].request_id }).result.outcome).toBe("unknown");
    expect(present(view, "identity-pending")).toBe(false);
    fireEvent.click(view.getByTestId("identity-cancel"));

    // A request left waiting stays in the roster with its withdrawal after
    // the form is closed.
    form = await openNewAgent(view);
    fireEvent.click(within(form).getByTestId("identity-profile:reviewer"));
    setField(form, "identity-field:name", "censor");
    setField(form, "identity-field:display_name", "The censor");
    fireEvent.submit(form);
    await find(view, "identity-notice");
    fireEvent.click(view.getByTestId("identity-cancel"));
    await gone(view, "identity-new-agent");
    const waiting = await find(view, "identity-pending:The censor");
    expect(waiting.textContent).toContain("The censor waits for a person's decision");
    expect(waiting.textContent).toMatch(/2030/);
    fireEvent.click(view.getByTestId("identity-pending-withdraw:The censor"));
    await gone(view, "identity-pending:The censor");
    expect(calls(family, "meta/identity/withdraw")).toHaveLength(2);
    expect(family.agents.has("censor")).toBe(false);
    view.unmount();
  }, 60_000);

  it("a_request_decided_or_withdrawn_elsewhere_no_longer_waits_in_the_open_form", async () => {
    // The console reads the roster again on its own clock; that clock is
    // the row's to turn.
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      const family = createScriptedIdentityFamily();
      const { view } = await openRoster(family);
      const ask = async (name: string, displayName: string): Promise<HTMLElement> => {
        const form = await openNewAgent(view);
        fireEvent.click(within(form).getByTestId("identity-profile:reviewer"));
        setField(form, "identity-field:name", name);
        setField(form, "identity-field:display_name", displayName);
        fireEvent.submit(form);
        const notice = await find(view, "identity-notice");
        expect(notice.querySelector("[data-outcome='pending_decision']")).not.toBeNull();
        expect(within(notice).queryByTestId("identity-withdraw")).not.toBeNull();
        return form;
      };
      const settled = async (): Promise<HTMLElement> => {
        await waitFor(() => expect(view.getByTestId("identity-notice").querySelector("[data-outcome='settled']")).not.toBeNull(), { timeout: 5_000 });
        return view.getByTestId("identity-notice");
      };

      // Withdrawn from the roster's list while the form still shows it: the
      // form stops saying that a person must decide, and offers no
      // withdrawal of a request that is gone.
      let form = await ask("censor", "The censor");
      fireEvent.click(await find(view, "identity-pending-withdraw:The censor"));
      let notice = await settled();
      expect(noticeWords(notice)).toContain("The request is no longer waiting for a decision");
      expect(noticeWords(notice)).not.toContain("A person must decide");
      expect(within(notice).queryByTestId("identity-withdraw")).toBeNull();
      // Nothing is left to send: the form does not come back.
      expect(within(form).queryByTestId("identity-submit")).toBeNull();
      expect(calls(family, "meta/identity/withdraw")).toHaveLength(1);
      fireEvent.click(view.getByTestId("identity-cancel"));
      await gone(view, "identity-new-agent");

      // Decided by a person: the console's next read finds the change made.
      form = await ask("auditor", "The auditor");
      family.decide("decision-2", true);
      await act(async () => {
        vi.advanceTimersByTime(15_000);
      });
      await find(view, "roster-row:auditor");
      notice = await settled();
      expect(within(notice).queryByTestId("identity-withdraw")).toBeNull();
      expect(calls(family, "meta/identity/withdraw")).toHaveLength(1);
      expect(calls(family, "meta/identity/create")).toHaveLength(2);
      view.unmount();
    } finally {
      vi.useRealTimers();
    }
  }, 60_000);

  it("a_retry_of_one_action_reuses_its_request_id", async () => {
    const family = createScriptedIdentityFamily();
    const { view } = await openRoster(family);
    const form = await openNewAgent(view);
    fireEvent.click(within(form).getByTestId("identity-profile:worker"));
    setField(form, "identity-field:name", "scout");

    // The create is carried out and its answer is lost: trying again sends
    // the same request under the same id, and nothing is made twice.
    family.scriptNext("meta/identity/create", { fail: "after" });
    fireEvent.submit(form);
    let notice = await find(view, "identity-notice");
    expect(notice.querySelector("[data-outcome='failed']")?.textContent).toBe("The console got no answer");
    expect(noticeWords(notice)).toContain("sends the same request");
    fireEvent.click(within(notice).getByTestId("identity-retry"));
    await waitFor(() => expect(noticeWords(view.getByTestId("identity-notice"))).toContain("scout is created"), { timeout: 5_000 });
    let created = calls(family, "meta/identity/create");
    expect(created).toHaveLength(2);
    expect(created[1]).toEqual(created[0]);
    expect([...family.agents.keys()].filter((name) => name === "scout")).toHaveLength(1);
    fireEvent.click(view.getByTestId("identity-cancel"));

    // A refusal that may be retried offers it, under the same id.
    let again = await openNewAgent(view);
    fireEvent.click(within(again).getByTestId("identity-profile:worker"));
    setField(again, "identity-field:name", "ranger");
    family.scriptNext("meta/identity/create", { refuse: { kind: "changes_held" } });
    fireEvent.submit(again);
    notice = await find(view, "identity-notice");
    expect(notice.querySelector("[data-outcome='refused']")?.textContent)
      .toBe("The host has paused changes of agents for now. Try again later.");
    expect(detailsOf(notice)).toBe("The host holds changes of agents for now: retry the same request later.");
    fireEvent.click(within(notice).getByTestId("identity-retry"));
    await waitFor(() => expect(noticeWords(view.getByTestId("identity-notice"))).toContain("ranger is created"), { timeout: 5_000 });
    created = calls(family, "meta/identity/create");
    expect(created).toHaveLength(4);
    expect(created[3].request_id).toBe(created[2].request_id);
    // Another action is another request.
    expect(created[2].request_id).not.toBe(created[0].request_id);
    fireEvent.click(view.getByTestId("identity-cancel"));

    // A request the person changed after a refusal takes a new id.
    again = await openNewAgent(view);
    fireEvent.click(within(again).getByTestId("identity-profile:worker"));
    setField(again, "identity-field:name", "warden");
    fireEvent.submit(again);
    await find(view, "identity-field-error:name");
    setField(again, "identity-field:name", "sentry");
    fireEvent.submit(again);
    await waitFor(() => expect(noticeWords(view.getByTestId("identity-notice"))).toContain("sentry is created"), { timeout: 5_000 });
    created = calls(family, "meta/identity/create");
    expect(created).toHaveLength(6);
    expect(created[5].request_id).not.toBe(created[4].request_id);
    fireEvent.click(view.getByTestId("identity-cancel"));

    // A removal whose answer is lost: trying again sends the request that
    // was read once, and the agent, gone by then, is not read again.
    await selectAgent(view, "sentry");
    fireEvent.click(view.getByTestId("roster-action:sentry:remove"));
    const confirm = await find(view, "identity-confirm:sentry:remove");
    family.scriptNext("meta/identity/remove", { fail: "after" });
    const readsBefore = calls(family, "meta/identity/get").filter((params) => params.name === "sentry").length;
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    notice = await find(view, "identity-notice");
    expect(notice.querySelector("[data-outcome='failed']")).not.toBeNull();
    fireEvent.click(within(notice).getByTestId("identity-retry"));
    // The roster names an agent without a display name by its name in words.
    await waitFor(() => expect(noticeWords(view.getByTestId("identity-notice"))).toContain("Sentry is removed"), { timeout: 5_000 });
    const removed = calls(family, "meta/identity/remove");
    expect(removed).toHaveLength(2);
    expect(removed[1]).toEqual(removed[0]);
    expect(calls(family, "meta/identity/get").filter((params) => params.name === "sentry").length).toBe(readsBefore + 1);
    view.unmount();
  }, 60_000);
});
