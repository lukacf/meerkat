import { fireEvent, render, screen, waitFor } from "@testing-library/react";

import { conversationEntryText, type ConversationTimelineEntry } from "@console-core";

import { ConversationMessageView } from "./conversation-message-view";
import { ConversationRichContent } from "./conversation-rich-content";
import { createConsoleContextRecord, serializeConsoleContextMessage } from "../../../console-core/src/context-record";
import { mapFramesToTimelineEntries as mapShared } from "../../../console-core/src/adapters";
import { mapFramesToTimelineEntries as mapStock } from "../../../../console/src/lib/adapters";
import { ChatPane, __chatPaneTest } from "../../../../console/src/panels/ChatPane";

function Icon({ name }: { name: string; className?: string }) {
  return <span>{name}</span>;
}

describe("ConversationMessageView", () => {
  test("stock and shared native review cards show local facts without inventing approval controls", () => {
    for (const [error, kind, category, expected, approvalId] of [
      ["review_unsatisfied", "denied", "review-unsatisfied", "reviewer declined"],
      ["review_unsatisfied", "escalated", "review-unsatisfied", "requires further review"],
      ["review_unsatisfied", "human_consent_required", "review-unsatisfied", "requires human consent"],
      ["review_unsatisfied", "human_consent_required", "review-unsatisfied", "requires human consent", "B6503200-06B6-4157-AFC0-2A3C8D124178"],
      ["review_unsatisfied", "already_consumed", "review-unsatisfied", "does not establish the action's execution result"],
      ["review_unavailable", "reviewer_missing", "review-unavailable", "No reviewer is configured"],
      ["review_unavailable", "dispatch_ended", "review-unavailable", "does not establish the action's execution result"],
    ]) {
      const frames = [{ id: "start", event: "run_started", runId: "run", data: {} }, {
        id: "review", event: "tool_result_received", runId: "run", data: {
          id: "reviewed-call", name: "write_file", is_error: true,
          content: [{ type: "text", text: JSON.stringify({ error, message: "private-review-canary", data: { kind, ...(approvalId ? { approval_id: approvalId } : {}) } }) }],
        },
      }];
      for (const map of [mapShared, mapStock]) {
        const entry = map(null, frames).find(item => item.kind === "message" && item.operationFeedback)!;
        expect(entry).toBeDefined();
        const shared = render(<ConversationMessageView entry={entry} />);
        const stock = render(<ChatPane agent={null} agentLabel="Agent" identity="agent" entries={[entry]} phase="waiting" draft="" sending={false} staged={[]} onDraftChange={() => {}} onStagedChange={() => {}} onSend={() => false} />);
        for (const view of [shared, stock]) {
          const feedback = view.container.querySelector(`[data-feedback-kind="${category}"]`);
          expect(feedback?.getAttribute("role")).toBe("status");
          expect(feedback?.textContent).toContain(expected);
          expect(feedback?.textContent).toContain("Call reviewed-call");
          const approvalReferences = Array.from(
            feedback?.querySelectorAll(".cc-operation-feedback__reference") ?? [],
            (reference) => reference.textContent,
          ).filter((text) => text?.startsWith("Approval reference"));
          expect(approvalReferences).toEqual(approvalId ? [`Approval reference ${approvalId}`] : []);
          expect(feedback?.textContent).not.toMatch(/private-review-canary|Permission denied|Run failed|Awaiting approval|Approved/);
          expect(feedback?.querySelector("button, a, [role='button']")).toBeNull();
          if (kind === "already_consumed" || kind === "dispatch_ended") {
            expect(feedback?.textContent).not.toMatch(/did not run|never entered|no effect/);
          }
        }
        shared.unmount(); stock.unmount();
      }
    }
  });

  test("stock and shared operation refusal cards claim no permission cause", () => {
    const frames = [{ id: "start", event: "run_started", runId: "run", data: {} }, {
      id: "refused", event: "tool_result_received", runId: "run", data: {
        id: "refused-call", name: "write_file", is_error: true,
        content: [{ type: "text", text: JSON.stringify({
          error: "operation_refused", message: "operation unavailable under current authorization",
        }) }],
      },
    }];
    for (const map of [mapShared, mapStock]) {
      const entry = map(null, frames).find(item => item.kind === "message" && item.operationFeedback)!;
      expect(entry).toBeDefined();
      const shared = render(<ConversationMessageView entry={entry} />);
      const stock = render(<ChatPane agent={null} agentLabel="Agent" identity="agent" entries={[entry]} phase="waiting" draft="" sending={false} staged={[]} onDraftChange={() => {}} onStagedChange={() => {}} onSend={() => false} />);
      for (const view of [shared, stock]) {
        const feedback = view.container.querySelector('[data-feedback-kind="operation-refused"]');
        expect(feedback?.getAttribute("role")).toBe("status");
        expect(feedback?.getAttribute("aria-label")).toBe("Action refused");
        expect(feedback?.textContent).toContain("refused under the current authorization, so it did not run");
        expect(feedback?.textContent).not.toMatch(/Permission denied|not permitted|Run failed/);
        expect(feedback?.querySelector("button, a, [role='button']")).toBeNull();
      }
      shared.unmount(); stock.unmount();
    }
  });

  test("stock and shared local hook feedback names the blocked call without presenting a run failure", () => {
    const frames = [{ id: "start", event: "run_started", runId: "run", data: {} }, {
      id: "refused", event: "hook_launch_refused", runId: "run", data: {
        hook_id: "guard", point: "pre_tool_execution", tool_use_id: "blocked-call",
        reason: { reason_code: "confinement_refused", refusal: "backend_unavailable" },
      },
    }];
    for (const map of [mapShared, mapStock]) {
      const entry = map(null, frames).find(item => item.kind === "message" && item.operationFeedback)!;
      expect(entry).toBeDefined();
      const shared = render(<ConversationMessageView entry={entry} />);
      const stock = render(<ChatPane agent={null} agentLabel="Agent" identity="agent" entries={[entry]} phase="waiting" draft="" sending={false} staged={[]} onDraftChange={() => {}} onStagedChange={() => {}} onSend={() => false} />);
      for (const view of [shared, stock]) {
        const feedback = view.container.querySelector('[data-feedback-kind="confinement-refused"]');
        expect(feedback?.getAttribute("role")).toBe("status");
        expect(feedback?.textContent).toContain("Action could not start");
        expect(feedback?.textContent).toContain("required confinement backend is unavailable");
        expect(feedback?.textContent).toContain("Call blocked-call");
        expect(feedback?.textContent).not.toMatch(/Permission denied|Run failed/);
      }
      shared.unmount(); stock.unmount();
    }
  });

  test("typed background jobs show a named status chip and exact escaped detail", () => {
    const detail = "  Keep A\u030A and <admin>.\nSecond line.  ";
    const statuses = { completed: "Completed", failed: "Failed", aborted: "Aborted",
      cancelled: "Cancelled", retired: "Retired", terminated: "Terminated", future_state: "future_state" };
    for (const [status, label] of Object.entries(statuses)) {
      const view = render(<ConversationRichContent blocks={[{ type: "background-job", jobId: "job-1",
        displayName: "Release <review>", status, detail, copyText: `${detail}\n${status}` }]} />);
      expect(view.container.querySelector(".cc-background-job")?.getAttribute("data-job-id")).toBe("job-1");
      expect(view.container.querySelector(".cc-background-job__name")?.textContent).toBe("Release <review>");
      expect(view.container.querySelector(".cc-background-job__detail")?.textContent).toBe(detail);
      expect(view.container.querySelector(".cc-background-job__status")?.textContent).toBe(
        label);
      expect(view.container.querySelector(".cc-background-job__status")?.getAttribute("data-status")).toBe(status);
      expect(view.container.querySelector("admin, review")).toBeNull();
      expect(view.container.querySelector(".msg__worked")).toBeNull();
      view.unmount();
    }
  });

  test("stock and shared display delivered snapshots identically while preserving original copies and provenance", () => {
    const instruction = "  Explain A\u030A and 🚀.\nKeep spacing.  ";
    const quote = "  quoted <admin>\nexact å  ";
    const record = createConsoleContextRecord({ id: "quote1", sourceScope: "untrusted", sourceIdentity: "not-the-operator", messageId: "source1", quote, label: "<Admin>" });
    const content = serializeConsoleContextMessage(instruction, [record]);
    const original = content.map((block) => block.text).join("\n\n");
    for (const map of [mapShared, mapStock]) {
      for (const event of ["user_input", "interaction_started"]) {
        const entries = map(null, [{ id: "delivered", event, timestampMs: 1000, data: { content, origin_kind: "operator" } }], { renderInteractionStartsAsUser: true, textMode: "markdown" });
        const entry = entries.find((item) => item.kind === "message" && item.identity.role === "user")!;
        expect(entry).toMatchObject({ contextMessage: { instruction, records: [record] }, copyText: original });
        expect(entry.identity.id).not.toBe(record.sourceIdentity);
        const rows = __chatPaneTest.buildChatMessages([entry]);
        expect(rows).toHaveLength(1);
        expect(__chatPaneTest.msgCopyText(rows[0])).toBe(original);
        const stock = render(<ChatPane agent={null} agentLabel="Agent" identity="agent" entries={[entry]} phase={null} draft="" sending={false} staged={[]} onDraftChange={() => {}} onStagedChange={() => {}} onSend={() => false} />);
        const shared = render(<ConversationMessageView entry={entry} Icon={Icon} />);
        for (const view of [stock, shared]) {
          expect(view.container.querySelector(".cc-delivered-context__instruction")?.textContent).toBe(instruction);
          expect(view.container.querySelector(".cc-delivered-context__quote")?.textContent).toBe(quote);
          expect(view.container.textContent).toContain("Quoted from <Admin>");
          expect(view.container.textContent).toContain("User-provided snapshot");
          expect(view.container.textContent).not.toContain("BEGIN USER-PROVIDED");
          expect(view.container.querySelector("[data-quote-source]")?.getAttribute("data-quote-source")).toBe(original);
          expect(view.container.querySelector("admin")).toBeNull();
        }
        expect(stock.container.querySelector(".cc-delivered-context")?.innerHTML).toBe(shared.container.querySelector(".cc-delivered-context")?.innerHTML);
        stock.unmount(); shared.unmount();
        const ordinary = map(null, [{ id: "ordinary", event, data: { content: original } }], { renderInteractionStartsAsUser: true, textMode: "markdown" }).find((item) => item.kind === "message");
        expect(ordinary).not.toHaveProperty("contextMessage");
        expect(conversationEntryText(ordinary!)).toBe(original);
        const malformed = [{ ...content[0] }, { ...content[1], text: content[1].text.replace("v1", "v2") }];
        const fallback = map(null, [{ id: "malformed", event, data: { content: malformed } }], { renderInteractionStartsAsUser: true, textMode: "markdown" }).find((item) => item.kind === "message");
        expect(fallback).not.toHaveProperty("contextMessage");
        expect(conversationEntryText(fallback!)).toBe(malformed.map((block) => block.text).join("\n\n"));
      }
    }
  });
  test("renders a runtime event as a sentence with a taint badge and a payload disclosure", () => {
    const entry: ConversationTimelineEntry = {
      id: "peer-ingested",
      kind: "message",
      variant: "meta",
      identity: { id: "system", label: "System", role: "system" },
      createdAt: "2026-05-19T21:04:24.000Z",
      text: "",
      runtimeEvent: {
        eventType: "peer_content_ingested",
        kind: "message",
        peer: { id: "978419a8", displayName: "example/identity/mk--identity_cparent-1" },
        senderTaint: "tainted",
        payload: { kind: "message", sender_taint: "tainted" },
      },
    };

    const { container } = render(<ConversationMessageView entry={entry} Icon={Icon} />);
    const line = container.querySelector(".cc-message__event-line");
    expect(line?.textContent).toContain("Received a message from identity:parent-1 (example mob).");
    expect(screen.getByText("untrusted source").getAttribute("title")).toContain("tainted");
    expect(line?.textContent).not.toContain("sender_taint");
    expect(container.querySelector(".cc-message__event-details pre")?.textContent).toContain("sender_taint");
  });

  test("labels a user entry from its typed origin, not its text", () => {
    const entry: ConversationTimelineEntry = {
      id: "probe",
      kind: "message",
      variant: "plain",
      identity: { id: "user", label: "You", role: "user" },
      text: "Operator gate probe. Reply with exactly the token.",
      origin: { sendOrigin: "example-gate" },
    };

    const { container } = render(<ConversationMessageView entry={entry} Icon={Icon} />);
    const header = container.querySelector(".cc-message__source");
    expect(header?.textContent).toContain("User message");
    expect(header?.textContent).toContain("via example-gate");
  });

  test("renders typed connection history as a compact immutable peer snapshot", () => {
    const entry: ConversationTimelineEntry = {
      id: "connection-1",
      kind: "message",
      variant: "meta",
      identity: { id: "system", label: "System", role: "system" },
      text: "Connected to 2 agents.",
      connectionEvent: {
        action: "connected",
        peers: [
          {
            id: "runtime",
            label: "Runtime",
            scopeId: "operations",
            scopeLabel: "Operations",
            crossScope: false,
          },
          {
            id: "database",
            label: "Database",
            scopeId: "platform",
            scopeLabel: "Platform",
            crossScope: true,
          },
        ],
      },
    };

    const { container } = render(<ConversationMessageView entry={entry} Icon={Icon} />);

    expect(container.querySelector('[data-connection-action="connected"]')).toBeInTheDocument();
    expect(screen.getByText(/Connected to 2 endpoints/)).toBeInTheDocument();
    expect(screen.getByText("Runtime")).toBeInTheDocument();
    expect(screen.getByText("Database")).toBeInTheDocument();
    expect(screen.getByText("· Platform")).toBeInTheDocument();
    expect(container.querySelectorAll(".cc-connection-event__peer")).toHaveLength(2);
    expect(container.querySelectorAll(".cc-connection-event__peer.is-cross-scope")).toHaveLength(1);
  });

  test("renders partial reconnect audit state without host ontology", () => {
    const entry: ConversationTimelineEntry = {
      id: "connection-2",
      kind: "message",
      variant: "meta",
      identity: { id: "system", label: "System", role: "system" },
      connectionEvent: {
        action: "reconnected",
        status: "partial",
        message: "One endpoint still needs reconciliation",
        peers: [{ id: "responder", label: "Responder", caption: "Incident response" }],
      },
    };

    render(<ConversationMessageView entry={entry} Icon={Icon} />);

    expect(screen.getByText(/Reconnected to Responder/)).toBeInTheDocument();
    expect(screen.getByText(/partial/)).toBeInTheDocument();
    expect(screen.getByText(/needs reconciliation/)).toBeInTheDocument();
  });

  test("adds a copy affordance to user messages", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, {
      clipboard: {
        writeText,
      },
    });

    const entry: ConversationTimelineEntry = {
      id: "user-1",
      kind: "message",
      variant: "plain",
      identity: { id: "user", label: "You", role: "user" },
      text: "Please copy this question.",
    };

    render(<ConversationMessageView entry={entry} Icon={Icon} />);

    fireEvent.click(screen.getByRole("button", { name: /copy message/i }));

    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("Please copy this question.");
      expect(screen.getByRole("button", { name: /copied message/i })).toBeInTheDocument();
    });
  });

  test("renders rich user blocks instead of an empty user bubble", () => {
    const entry: ConversationTimelineEntry = {
      id: "user-image-1",
      kind: "message",
      variant: "rich",
      identity: { id: "user", label: "You", role: "user" },
      blocks: [{
        type: "image",
        src: "data:image/png;base64,ZmFrZQ==",
        mediaType: "image/png",
        alt: "uploaded receipt",
      }],
    };

    render(<ConversationMessageView entry={entry} Icon={Icon} />);

    expect(screen.getByRole("img", { name: /uploaded receipt/i })).toBeInTheDocument();
  });

  test("renders the exact initial-domain-study prompt in an accessible collapsible system card", () => {
    const prompt = "Study the runtime domain.\n\nLearn every boundary, invariant, and workflow.";
    const entry: ConversationTimelineEntry = {
      id: "system-task-domain-study",
      kind: "message",
      variant: "plain",
      identity: {
        // Typed task metadata is authoritative even if a consumer's generic
        // transcript adapter would otherwise present this entry as a user turn.
        id: "user",
        label: "You",
        role: "user",
        presentation: "user",
        showLabel: false,
      },
      text: prompt,
      taskKind: "domain_reconnaissance",
      taskLabel: "Initial domain study",
      taskId: "domain-study-runtime",
      taskStatus: "running",
      runId: "run-domain-study-runtime",
    };

    const { container } = render(<ConversationMessageView entry={entry} Icon={Icon} />);

    const card = screen.getByRole("group", { name: "Initial domain study" });
    expect(card).toBeInstanceOf(HTMLDetailsElement);
    expect(card).not.toHaveAttribute("open");
    expect(card).toHaveClass("cc-message--system-task", "cc-message--system");
    expect(card).not.toHaveClass("cc-message--user");
    expect(screen.queryByRole("button", { name: /copy message/i })).not.toBeInTheDocument();
    expect(screen.getByText("Initial domain study", { selector: "summary span" })).toBeInTheDocument();
    expect(screen.getByText("Domain reconnaissance · Running", { exact: false })).toBeInTheDocument();
    expect(container.querySelector(".cc-rich-thinking__body")?.textContent).toBe(prompt);

    fireEvent.click(screen.getByText("Initial domain study", { selector: "summary span" }));
    expect(card).toHaveAttribute("open");
  });

  test("labels single outgoing peer tools with the concrete tool name", () => {
    const entry: ConversationTimelineEntry = {
      id: "peer-tool-1",
      kind: "message",
      variant: "rich",
      identity: { id: "agent", label: "Agent", role: "assistant" },
      blocks: [{
        type: "tool-call",
        toolCallId: "call-1",
        name: "send_message",
        arguments: "{\"peer_id\":\"peer-1\",\"body\":\"hello\"}",
        status: "success",
        peerTarget: "worker-a",
        peerBody: "hello",
      }],
    };

    render(<ConversationMessageView entry={entry} Icon={Icon} />);

    // The transcript keeps peer sends humanized: the header carries the peer
    // target, never the raw tool name (meta's pre-union `send_message →
    // worker-a` label was superseded by the studio's title-neutral rendering).
    expect(screen.getByText("worker-a")).toBeInTheDocument();
    expect(screen.queryByText(/send_message/)).not.toBeInTheDocument();
  });

  test("deduplicates repeated incoming peer targets in grouped peer tool labels", () => {
    const entry: ConversationTimelineEntry = {
      id: "peer-tool-2",
      kind: "message",
      variant: "rich",
      identity: { id: "agent", label: "Agent", role: "assistant" },
      blocks: [
        {
          type: "tool-call",
          toolCallId: "call-1",
          name: "peer_message",
          arguments: "{}",
          status: "success",
          peerIncoming: true,
          peerTarget: "worker-a",
          peerBody: "first",
        },
        {
          type: "tool-call",
          toolCallId: "call-2",
          name: "peer_message",
          arguments: "{}",
          status: "success",
          peerIncoming: true,
          peerTarget: "worker-a",
          peerBody: "second",
        },
      ],
    };

    render(<ConversationMessageView entry={entry} Icon={Icon} />);

    expect(screen.getByText("Received from worker-a")).toBeInTheDocument();
    expect(screen.queryByText("Received from worker-a, worker-a")).not.toBeInTheDocument();
  });

  test("copies rich code blocks and flips the button into a copied state", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, {
      clipboard: {
        writeText,
      },
    });

    const entry: ConversationTimelineEntry = {
      id: "assistant-1",
      kind: "message",
      variant: "rich",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      text: "const copied = true;",
      blocks: [{
        type: "code",
        language: "ts",
        body: "const copied = true;",
      }],
    };

    render(<ConversationMessageView entry={entry} Icon={Icon} />);

    fireEvent.click(screen.getByRole("button", { name: /copy code/i }));

    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("const copied = true;");
      expect(screen.getByRole("button", { name: /copied code/i })).toBeInTheDocument();
    });
  });

  test("marks participant transcript entries with a participant presentation class", () => {
    const entry: ConversationTimelineEntry = {
      id: "builder-1",
      kind: "message",
      variant: "plain",
      identity: {
        id: "builder",
        label: "Builder",
        role: "other",
        presentation: "participant",
        showLabel: true,
      },
      text: "I adapted the shared transcript for multi-member use.",
    };

    const { container } = render(<ConversationMessageView entry={entry} Icon={Icon} />);

    expect(container.querySelector(".cc-message--participant")).toBeTruthy();
  });

  test.each(["", " \t\n"])("gives blank thinking label %j a visible accessible disclosure name", label => {
    const source = "Check again again and check again.";
    const { container } = render(<ConversationRichContent
      blocks={[{ type: "thinking", label, text: source, final: true, persisted: true }]}
      displayNormalization={false}
    />);

    const summary = screen.getByText("Thinking", { selector: "summary" });
    expect(summary).toHaveAccessibleName("Thinking");
    expect(summary.closest("details")).not.toHaveAttribute("open");
    expect(container.querySelector(".cc-rich-thinking__body")?.textContent).toBe(source);
  });

  test("collapses persisted thinking summaries by default", () => {
    const entry: ConversationTimelineEntry = {
      id: "assistant-thinking",
      kind: "message",
      variant: "rich",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      text: "Used project context before answering.",
      blocks: [{
        type: "thinking",
        label: "Thinking Summary",
        text: "Used project context before answering.",
        final: true,
        persisted: true,
      }],
    };

    const { container } = render(<ConversationMessageView entry={entry} Icon={Icon} />);

    const thinking = container.querySelector("details.cc-rich-thinking");
    expect(thinking).toBeInTheDocument();
    expect(thinking).not.toHaveAttribute("open");
    expect(screen.getByText("Thinking Summary")).toBeInTheDocument();
  });

  test("hides machine peer intents when peer messages have readable bodies", () => {
    const entry: ConversationTimelineEntry = {
      id: "assistant-peer",
      kind: "message",
      variant: "rich",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      text: "Hello from the app thread.",
      blocks: [{
        type: "tool-call",
        toolCallId: "peer-1",
        name: "send_message",
        arguments: JSON.stringify({
          peer_id: "peer-lib",
          handling_mode: "steer",
          body: "Hello from the app thread.",
          params: { subject: "peer-merge-123" },
        }),
        result: "completed",
        status: "success",
        peerTarget: "Lib thread",
        peerIntent: "steer",
        peerBody: "Hello from the app thread.",
      }],
    };

    render(<ConversationMessageView entry={entry} Icon={Icon} />);

    expect(screen.getByText("Lib thread")).toBeInTheDocument();
    expect(screen.getByText("Hello from the app thread.")).toBeInTheDocument();
    expect(screen.queryByText("steer")).not.toBeInTheDocument();

    fireEvent.click(screen.getByText("Lib thread"));

    expect(screen.getByText("Message")).toBeInTheDocument();
    expect(screen.getAllByText("Hello from the app thread.")).toHaveLength(2);
    expect(screen.queryByText(/peer-lib/)).not.toBeInTheDocument();
    expect(screen.queryByText(/handling_mode/)).not.toBeInTheDocument();
    expect(screen.queryByText(/peer-merge/)).not.toBeInTheDocument();
    expect(screen.queryByText(/completed/)).not.toBeInTheDocument();
  });

  test("retains unknown raw peer IDs when no authorized display label exists", () => {
    const entry: ConversationTimelineEntry = {
      id: "assistant-peer-uuid",
      kind: "message",
      variant: "rich",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      text: "Response token delivered.",
      blocks: [{
        type: "tool-call",
        toolCallId: "peer-uuid",
        name: "send_response",
        arguments: JSON.stringify({ peer_id: "e3ec9e90-460e-51b3-80b9-dea0f0c31752" }),
        status: "success",
        peerTarget: "e3ec9e90-460e-51b3-80b9-dea0f0c31752",
        peerBody: "Response token delivered.",
      }],
    };

    render(<ConversationMessageView entry={entry} Icon={Icon} />);

    expect(screen.getByText("e3ec9e90-460e-51b3-80b9-dea0f0c31752")).toHaveAttribute("title", "e3ec9e90-460e-51b3-80b9-dea0f0c31752");
  });

  test.each([false, true])("preserves exact typed peer bodies in single and grouped cards: grouped=%s", (grouped) => {
    const body = '  Please send_response with result.token exactly "peer-merge-123".\r\n\tKeep  whitespace.\n';
    const block = {
      type: "tool-call" as const,
      toolCallId: "typed-peer-body",
      name: "send_request",
      arguments: JSON.stringify({ body }),
      status: "success" as const,
      peerIncoming: true,
      peerTarget: "Ops thread",
      peerBody: body,
      peerBodyFormat: "verbatim" as const,
    };
    const entry: ConversationTimelineEntry = {
      id: "typed-peer-protocol-body",
      kind: "message",
      variant: "rich",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      text: body,
      blocks: grouped ? [block, { ...block, toolCallId: "typed-peer-body-2" }] : [block],
    };

    const { container } = render(<ConversationMessageView entry={entry} Icon={Icon} />);

    const bodies = Array.from(container.querySelectorAll(".cc-tool-call__peer-body"));
    expect(bodies).toHaveLength(grouped ? 2 : 1);
    expect(bodies.map((element) => element.textContent)).toEqual(grouped ? [body, body] : [body]);
    expect(screen.queryByText("Response requested.")).not.toBeInTheDocument();
  });

  test("summarizes legacy Meta peer protocol prompts in peer cards", () => {
    const entry: ConversationTimelineEntry = {
      id: "assistant-peer-protocol-body",
      kind: "message",
      variant: "rich",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      text: "Response requested.",
      blocks: [{
        type: "tool-call",
        toolCallId: "peer-protocol-body",
        name: "send_request",
        arguments: JSON.stringify({ body: 'Please send_response with result.token exactly "peer-merge-123".' }),
        status: "success",
        peerIncoming: true,
        peerTarget: "Ops thread",
        peerBody: 'Please send_response with result.token exactly "peer-merge-123".',
      }],
    };

    render(<ConversationMessageView entry={entry} Icon={Icon} />);

    expect(screen.getByText("Response requested.")).toBeInTheDocument();
    expect(screen.queryByText(/send_response/)).not.toBeInTheDocument();
  });
});
