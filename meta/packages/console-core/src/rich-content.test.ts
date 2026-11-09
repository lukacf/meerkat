import { describe, expect, test } from "./test-support/vitest-shim";
import assert from "node:assert/strict";

import {
  buildConversationMarkdownBlocks,
  conversationRichBlockCopyText,
  conversationRichBlocksToText,
  conversationRichPeerBodyForDisplay,
  conversationRichPeerIntentForDisplay,
  conversationRichPeerTargetForDisplay,
  normalizeConversationDisplayLabel,
  normalizeProjectDisplayLabel,
  parseConversationRichBlocks,
  parseConversationSummary,
  parseStreamingConversationRichBlocks,
  renderConversationInlineMarkdown,
  safeConsoleHref,
  normalizeConversationDisplayText,
} from "./rich-content";

describe("inline markdown", () => {
  test("keeps intra-word underscores literal", () => {
    expect(renderConversationInlineMarkdown("MEERKAT_TOUR_OK")).toBe("MEERKAT_TOUR_OK");
    expect(renderConversationInlineMarkdown("snake_case_name and __dunder__")).toBe(
      "snake_case_name and __dunder__",
    );
  });

  test("still emphasizes standalone underscore spans", () => {
    expect(renderConversationInlineMarkdown("a _word_ here")).toBe("a <em>word</em> here");
  });
});

describe("conversation rich-content parser", () => {
  test("parses markdown tables into structured table blocks", () => {
    const blocks = parseConversationRichBlocks(`
| Surface | Status | Notes |
| :--- | :---: | ---: |
| Transcript | done | 1 |
| Sidebar | next | 2 |
    `);

    expect(blocks).toHaveLength(1);
    expect(blocks[0]).toMatchObject({
      type: "table",
      headers: ["Surface", "Status", "Notes"],
      alignments: ["left", "center", "right"],
      rows: [
        ["Transcript", "done", "1"],
        ["Sidebar", "next", "2"],
      ],
    });
  });

  test("parses summary paragraphs into structured summary metadata", () => {
    const summary = parseConversationSummary(`
2 files changed +12 -3
packages/console-core/src/conversation.ts +8 -0
desktop/renderer/src/app/App.tsx +4 -3
    `);

    expect(summary).toMatchObject({
      title: "2 files changed",
      plus: 12,
      minus: 3,
      files: [
        { name: "packages/console-core/src/conversation.ts", plus: 8, minus: 0 },
        { name: "desktop/renderer/src/app/App.tsx", plus: 4, minus: 3 },
      ],
    });
  });

  test("restores inline code after emphasis parsing", () => {
    expect(renderConversationInlineMarkdown("Use `config/mob.toml` and `meta/send_message`.")).toBe(
      'Use <code class="cc-rich-inline-code">config/mob.toml</code> and <code class="cc-rich-inline-code">meta/send_message</code>.',
    );
  });

  test("does not let inline-code placeholders collide with user text", () => {
    expect(renderConversationInlineMarkdown("Literal @@CODETOKEN0@@ and `meta/send_message`.")).toBe(
      'Literal @@CODETOKEN0@@ and <code class="cc-rich-inline-code">meta/send_message</code>.',
    );
  });

  test("neutralizes legacy leaked code placeholders", () => {
    expect(renderConversationInlineMarkdown("@@CODE0@@ builds runtime and exposes @@CODE1@@.")).toBe(
      "builds runtime and exposes.",
    );
  });

  test("neutralizes machine peer tokens inside inline code", () => {
    expect(renderConversationInlineMarkdown("Peer response received and verified: `peer-merge-123`.")).toBe(
      "Peer response received and verified.",
    );
  });

  test("preserves public Meta peer message kinds in inline code", () => {
    const text = "All are connected over **inproc** and support `peer_message`, `peer_request`, and `peer_response`.";
    const blocks = parseConversationRichBlocks(text);

    expect(blocks).toEqual([{ type: "paragraph", text }]);
    expect(renderConversationInlineMarkdown(text)).toBe(
      "All are connected over <strong>inproc</strong> and support "
        + '<code class="cc-rich-inline-code">peer_message</code>, '
        + '<code class="cc-rich-inline-code">peer_request</code>, and '
        + '<code class="cc-rich-inline-code">peer_response</code>.',
    );
  });

  test("normalizes legacy leaked code placeholders before rich block parsing", () => {
    const blocks = parseConversationRichBlocks([
      "@@CODE0@@ — builds Meta runtime, starts channels/connectors, exposes @@CODE1@@, @@CODE2@@, @@CODE3@@, and proxies stock console/runtime routes.",
      "@@CODE4@@ — turns @@CODE5@@ + config/mob.toml into durable identities and peer topology.",
    ].join("\n"));

    expect(blocks).toEqual([{
      type: "paragraph",
      text: [
        "builds Meta runtime, starts channels/connectors, exposes and proxies stock console/runtime routes.",
        "turns config/mob.toml into durable identities and peer topology.",
      ].join("\n"),
    }]);
    expect(conversationRichBlocksToText(blocks)).not.toContain("@@CODE");
    expect(conversationRichBlocksToText(blocks)).not.toContain("...");
  });

  test("drops orphan punctuation fragments from normalized assistant paragraphs", () => {
    const blocks = parseConversationRichBlocks("Delivered the peer smoke message.\n\n.”");

    expect(blocks).toEqual([{
      type: "paragraph",
      text: "Delivered the peer smoke message.",
    }]);
  });

  test("omits machine peer intents from plain text copies when a message body is present", () => {
    expect(conversationRichBlocksToText([{
      type: "tool-call",
      toolCallId: "peer-1",
      name: "send_message",
      arguments: "{}",
      status: "success",
      peerTarget: "Lib thread",
      peerIntent: "steer",
      peerBody: "Hello from the app thread.",
    }])).toBe("→ to Lib thread: Hello from the app thread.");
  });

  test("does not expose raw UUID peer targets as display labels", () => {
    expect(conversationRichPeerTargetForDisplay("e3ec9e90-460e-51b3-80b9-dea0f0c31752")).toBe("Peer");
    expect(conversationRichPeerTargetForDisplay("Lib thread")).toBe("Lib thread");
    expect(conversationRichPeerTargetForDisplay("Ops peer source peer-root-1781853922227")).toBe("Ops peer thread");
  });

  test("normalizes legacy machine-token labels before shared console rendering", () => {
    expect(normalizeConversationDisplayLabel("Example peer target peer-root-1781853922227")).toBe("Example peer thread");
    expect(normalizeConversationDisplayLabel("Ops request source peer-req-1781854233913")).toBe("Ops request thread");
    expect(normalizeConversationDisplayLabel("Example merged response peer-merge-1781854428883")).toBe("Example peer response");
    expect(normalizeConversationDisplayLabel("Peer live ops peer-live-1781853108762")).toBe("Ops peer thread");
    expect(normalizeConversationDisplayLabel("e3ec9e90-460e-51b3-80b9-dea0f0c31752")).toBe("");
    expect(normalizeConversationDisplayLabel("Design review")).toBe("Design review");
  });

  test("formats project labels without changing ordinary conversation labels", () => {
    expect(normalizeProjectDisplayLabel("ops_clean")).toBe("Ops Clean");
    expect(normalizeProjectDisplayLabel("example")).toBe("Example");
    expect(normalizeProjectDisplayLabel("meerkat-app")).toBe("Meerkat App");
    expect(normalizeConversationDisplayLabel("Design review")).toBe("Design review");
  });

  test("hides machine peer correlation tokens from intent chrome", () => {
    expect(conversationRichPeerIntentForDisplay("peer-req-1781854233913")).toBeUndefined();
    expect(conversationRichPeerIntentForDisplay("e3ec9e90-460e-51b3-80b9-dea0f0c31752")).toBeUndefined();
    expect(conversationRichPeerIntentForDisplay("design-review")).toBe("design-review");
  });

  test("preserves authored peer bodies without protocol guesses", () => {
    for (const source of [
      'Please send_response with result.token exactly "peer-merge-123".',
      "Please reply with ACK_FROM_PEER_peer-root-123 and do not edit files.",
      "ACKFROMPEER_peer-root-123",
      "peer-merge-123",
      "e3ec9e90-460e-51b3-80b9-dea0f0c31752",
      "  Acknowledged from my thread: peer-live-token.\n\nKeep  whitespace.\n",
      "Peer response received and verified: `peer-merge-123`.",
    ]) {
      expect(conversationRichPeerBodyForDisplay(source)).toBe(source);
    }
    expect(conversationRichPeerBodyForDisplay("  ")).toBeUndefined();
    expect(conversationRichPeerBodyForDisplay(null)).toBeUndefined();
  });

  test("legacy peer format keeps protocol summaries without changing the source default", () => {
    const source = 'Please send_response with result.token exactly "peer-merge-123".';
    expect(conversationRichPeerBodyForDisplay(source, "legacy")).toBe("Response requested.");
    expect(conversationRichPeerBodyForDisplay(source, "verbatim")).toBe(source);
    expect(conversationRichPeerBodyForDisplay(source)).toBe(source);
  });

  test("legacy rich parser retains its explicit normalization compatibility", () => {
    expect(parseConversationRichBlocks("Please reply with ACK_FROM_PEER_peer-root-123 and do not edit files.")).toEqual([{
      type: "paragraph",
      text: "Please reply with acknowledgement and do not edit files.",
    }]);
    expect(parseConversationRichBlocks('Send result.token exactly "peer-merge-123".')).toEqual([{
      type: "paragraph",
      text: 'Send result.token exactly "response token".',
    }]);
    expect(parseConversationRichBlocks("Sent a peer message containing peer-live-1781853108762.")).toEqual([{
      type: "paragraph",
      text: "Sent a peer message.",
    }]);
  });

  test("summarizes generated peer steering prompts for display", () => {
    expect(parseConversationRichBlocks([
      "Connected to Example peer thread. Each thread keeps its own transcript and can message the other through Meta.",
      "",
      "Use your Meta peer tools only. Do not run shell commands and do not edit files.",
      "Send this exact message body to the peered Example thread: \"Please reply with acknowledgement and do not edit files.\"",
      "After the peer message is sent, stop.",
    ].join("\n"))).toEqual([{
      type: "paragraph",
      text: "Connected to Example peer thread.\nRequested an acknowledgement from Example peer thread.",
    }]);
    expect(parseConversationRichBlocks([
      "Call peers, then send_request with params {\"subject\":\"peer-merge-1781854428883\"}.",
      "Ask the peer to send_response with result.token exactly \"peer-merge-1781854428883\".",
    ].join("\n"))).toEqual([{
      type: "paragraph",
      text: "Requested a peer response.",
    }]);
    expect(parseConversationRichBlocks([
      "Connected to Example peer response. Each thread keeps its own transcript and can message the other through Meta.",
      "",
      "Use your Meta peer tools only. Do not run shell commands and do not edit files.",
      "Call peers, then send a send_request to the peered Example thread using intent checksum_token and params {\"subject\":\"response token\"}.",
      "In the request blocks, ask it to send_response with result.token exactly \"response token\".",
      "After the request is sent, stop.",
    ].join("\n"))).toEqual([{
      type: "paragraph",
      text: "Connected to Example peer response.\nRequested a peer response from Example peer response.",
    }]);
    expect(parseConversationRichBlocks([
      "Use your Meta peer tools only. Do not run shell commands and do not edit files.",
      "Send this exact message body to the peered Example thread: \"Please reply with acknowledgement and do not edit files.\"",
      "After the peer message is sent, stop.",
    ].join("\n"))).toEqual([{
      type: "paragraph",
      text: "Requested an acknowledgement from Example thread.",
    }]);
    expect(parseConversationRichBlocks([
      "Use your Meta peer tools only. Do not run shell commands and do not edit files.",
      "Call peers, then send a send_request to the peered Example thread using intent checksum_token and params {\"subject\":\"response token\"}.",
      "In the request blocks, ask it to send_response with result.token exactly \"response token\".",
      "After the request is sent, stop.",
    ].join("\n"))).toEqual([{
      type: "paragraph",
      text: "Requested a peer response from Example thread.",
    }]);
    expect(parseConversationRichBlocks("Use your Meta peer tools only. Do not run shell commands and do not edit files.")).toEqual([]);
    expect(parseConversationRichBlocks("Send this exact message body to the peered Example thread: \"Please reply with acknowledgement and do not edit files.\"")).toEqual([{
      type: "paragraph",
      text: "Requested an acknowledgement from Example thread.",
    }]);
    expect(parseConversationRichBlocks("Call peers, then send a send_request to the peered Example thread using intent checksum_token and params {\"subject\":\"response token\"}.")).toEqual([{
      type: "paragraph",
      text: "Requested a peer response from Example thread.",
    }]);
    expect(parseConversationRichBlocks([
      "Connected to Example peer thread. Each thread keeps its own transcript. They can now message each other.",
      "",
      "Find your trusted peer and use send_message with handling_mode steer to send this exact body: \"Meta live peer smoke. Please reply in your own thread in one sentence. If you can, send a one sentence peer message back. Do not edit files.\".",
      "Then stop after reporting delivery.",
    ].join("\n"))).toEqual([{
      type: "paragraph",
      text: "Connected to Example peer thread.\nRequested a peer reply from Example peer thread.",
    }]);
    expect(parseConversationRichBlocks("Find your trusted peer and use send_message with handling_mode steer to send this exact body: \"Hello\". Then stop after reporting delivery.")).toEqual([{
      type: "paragraph",
      text: "Sent a peer message.",
    }]);
    expect(parseConversationRichBlocks("In the request blocks, ask it to send_response with result.token exactly \"response token\".")).toEqual([]);
    expect(parseConversationRichBlocks("After the request is sent, stop.")).toEqual([]);
    expect(parseConversationRichBlocks("Peer response received and verified: .")).toEqual([{
      type: "paragraph",
      text: "Peer response received and verified.",
    }]);
  });
});

test("parseStreamingConversationRichBlocks renders only stable markdown blocks", () => {
  const blocks = parseStreamingConversationRichBlocks("## Risk\n\n- first\n- seco");

  assert.equal(blocks.length, 2);
  assert.deepEqual(blocks[0], {
    type: "heading",
    level: 2,
    text: "Risk",
  });
  assert.deepEqual(blocks[1], {
    type: "paragraph",
    text: "- first\n- seco",
    streaming: true,
  });
});

test("final rich parsing preserves the streamed paragraph topology", () => {
  const text = "First paragraph.\n\nThe final line is complete.";
  const streamed = parseStreamingConversationRichBlocks(text);
  const finalized = parseConversationRichBlocks(text);

  assert.deepEqual(streamed, [
    { type: "paragraph", text: "First paragraph." },
    { type: "paragraph", text: "The final line is complete.", streaming: true },
  ]);
  assert.deepEqual(finalized, [
    { type: "paragraph", text: "First paragraph." },
    { type: "paragraph", text: "The final line is complete." },
  ]);
});

test("parseStreamingConversationRichBlocks keeps unclosed fences as plain tail text", () => {
  const blocks = parseStreamingConversationRichBlocks("Intro\n\n```ts\nconst answer = 1");

  assert.equal(blocks.length, 2);
  assert.deepEqual(blocks[0], {
    type: "paragraph",
    text: "Intro",
  });
  assert.deepEqual(blocks[1], {
    type: "paragraph",
    text: "```ts\nconst answer = 1",
    streaming: true,
  });
});

test("parseStreamingConversationRichBlocks hides unfinished inline markdown spans", () => {
  const unfinished = parseStreamingConversationRichBlocks("Having *proper render");
  assert.equal(unfinished.length, 1);
  assert.deepEqual(unfinished[0], {
    type: "paragraph",
    text: "Having",
    streaming: true,
  });

  const finished = parseStreamingConversationRichBlocks("Having *proper rendering* is essential.");
  assert.equal(finished.length, 1);
  assert.deepEqual(finished[0], {
    type: "paragraph",
    text: "Having *proper rendering* is essential.",
    streaming: true,
  });
  assert.equal(
    renderConversationInlineMarkdown(finished[0]?.type === "paragraph" ? finished[0].text : ""),
    "Having <em>proper rendering</em> is essential.",
  );
});

test("parseStreamingConversationRichBlocks does not treat identifiers as unfinished markdown", () => {
  const blocks = parseStreamingConversationRichBlocks("Use api_investigator and ORDER_THREE_OK.");
  assert.equal(blocks.length, 1);
  assert.equal(blocks[0]?.type === "paragraph" ? blocks[0].text : "", "Use api_investigator and ORDER_THREE_OK.");
});

test("renderConversationInlineMarkdown does not italicize underscores inside identifiers", () => {
  assert.equal(renderConversationInlineMarkdown("ORDER_THREE_OK"), "ORDER_THREE_OK");
  assert.equal(renderConversationInlineMarkdown("api_investigator"), "api_investigator");
  assert.equal(renderConversationInlineMarkdown("Use _emphasis_ here."), "Use <em>emphasis</em> here.");
});

test("renderConversationInlineMarkdown allowlists link schemes", () => {
  assert.equal(
    renderConversationInlineMarkdown("[Docs](https://example.test/docs)"),
    '<a href="https://example.test/docs" rel="noreferrer">Docs</a>',
  );
  assert.equal(
    renderConversationInlineMarkdown("[Local](/console) [Unsafe](javascript:alert(1))"),
    // The nested-paren href stops at the first `)`; the dangling paren stays
    // as literal text once the unsafe link is neutralized to its label.
    '<a href="/console" rel="noreferrer">Local</a> Unsafe)',
  );
  assert.equal(
    renderConversationInlineMarkdown("[Data](data:text/plain;base64,abc)"),
    "Data",
  );
  assert.equal(safeConsoleHref("//evil.test/phish"), null);
  assert.equal(safeConsoleHref("mailto:ops@example.test\r\nbcc:evil@example.test"), null);
});

describe("splitMixedProseSection via parseConversationRichBlocks", () => {
  test("heading glued to prose with single newlines becomes a heading block", () => {
    const blocks = parseConversationRichBlocks("intro line\n## Bottom-line assessment\nThe ceiling is event amplification.");
    expect(blocks.map((block) => block.type)).toEqual(["paragraph", "heading", "paragraph"]);
    expect(blocks[1]).toMatchObject({ type: "heading", level: 2, text: "Bottom-line assessment" });
  });

  test("pipe table glued to surrounding prose renders as a table block", () => {
    const blocks = parseConversationRichBlocks([
      "# 5. Highest-risk design flaws",
      "| Risk | Impact |",
      "|---|---:|",
      "| Runaway concurrency | High |",
      "| Replay storm | High |",
      "The highest-leverage improvements are.",
    ].join("\n"));
    const types = blocks.map((block) => block.type);
    expect(types).toEqual(["heading", "table", "paragraph"]);
    const table = blocks[1] as { headers: string[]; rows: string[][] };
    expect(table.headers).toEqual(["Risk", "Impact"]);
    expect(table.rows).toHaveLength(2);
  });

  test("list markers in mixed sections render as bullets", () => {
    const blocks = parseConversationRichBlocks("## Plan\n- first\n- second");
    expect(blocks[1]).toMatchObject({ type: "paragraph", text: "\u2022 first\n\u2022 second" });
  });
});

describe("display-normalization sentinel (PRs #281-#290 audit regressions)", () => {
  test("a markdown rule between paragraphs never leaks sentinel text", () => {
    const blocks = parseConversationRichBlocks("para one\n\n---\n\npara two");
    const rendered = JSON.stringify(blocks);
    expect(rendered).not.toContain("CCPARAGRAPHBREAK");
    expect(rendered).toContain("para one");
    expect(rendered).toContain("para two");
  });

  test("a punctuation-only line between paragraphs never leaks sentinel text", () => {
    const blocks = parseConversationRichBlocks("intro\n\n...\n\ndetails");
    expect(JSON.stringify(blocks)).not.toContain("CCPARAGRAPHBREAK");
  });

  test("content literally containing the private-use sentinel cannot forge breaks", () => {
    const blocks = parseConversationRichBlocks("safe \uE000CCPARAGRAPHBREAK\uE001 text");
    const rendered = JSON.stringify(blocks);
    expect(rendered).not.toContain("\uE000");
    expect(rendered).toContain("safe");
    expect(rendered).toContain("text");
  });
});

describe("machine peer-token boundary (audit regression)", () => {
  test("generated tokens continuing a kind word are still scrubbed", () => {
    const scrubbed = normalizeConversationDisplayText("routed via peer_response-1a2b just now");
    expect(scrubbed).not.toContain("peer_response-1a2b");
  });

  test("the public kind words survive in prose", () => {
    const scrubbed = normalizeConversationDisplayText("use peer_message to reach a member");
    expect(scrubbed).toContain("peer_message");
  });
});


describe("whole Markdown documents", () => {
  test("retains exact source and stable identity through stream completion", () => {
    const source = "  # Heading\r\n\r\n- nested\n  - list\n\n[ref][late]\n\n[late]: https://example.test\n";
    const live = buildConversationMarkdownBlocks(source, { documentId: "message:text:0", streaming: true });
    const done = buildConversationMarkdownBlocks(source, { documentId: "message:text:0" });
    assert.deepEqual(live, [{ type: "markdown", id: "message:text:0", source, streaming: true }]);
    assert.deepEqual(done, [{ type: "markdown", id: "message:text:0", source, streaming: false }]);
    assert.equal(conversationRichBlockCopyText(done[0]), source);
    assert.equal(conversationRichBlocksToText(done), source);
  });

  test("does not infer execution cards from ordinary message text", () => {
    for (const source of ["Created src/main.rs +3 -1", "$ rm -rf example", '{"status":"success"}']) {
      assert.equal(buildConversationMarkdownBlocks(source)[0].type, "markdown");
      assert.equal(conversationRichBlockCopyText(buildConversationMarkdownBlocks(source)[0]), source);
    }
  });
});
