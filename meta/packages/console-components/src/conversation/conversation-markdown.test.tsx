import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

import { buildConversationMarkdownBlocks, conversationRichBlockCopyText, conversationEntryText } from "@console-core";
import { mapFramesToTimelineEntries } from "../../../console-core/src/adapters";
import { buildConversationViewState } from "../../../console-core/src/adapters";
import { ConversationMarkdown, resolveMarkdownImage, resolveMarkdownLink, splitMarkdownChunks } from "./conversation-markdown";
import { ConversationRichContent } from "./conversation-rich-content";
import { ConversationTranscript } from "./conversation-transcript";
import { MARKDOWN_CORPUS, markdownEditSequence } from "./markdown-corpus";

function normalizedDom(input: Node): unknown {
  const node = input.cloneNode(true);
  node.normalize();
  if (node.nodeType === Node.TEXT_NODE) return node.textContent;
  if (!(node instanceof Element)) return null;
  return {
    tag: node.tagName,
    attributes: [...node.attributes].map((attribute) => [
      attribute.name,
      attribute.name === "style" ? (node as HTMLElement).style.cssText : attribute.value,
    ]).sort(([a], [b]) => a.localeCompare(b)),
    children: [...node.childNodes].map(normalizedDom),
  };
}

const doc = (source: string, streaming = false) => buildConversationMarkdownBlocks(source, { documentId: "row:text:0", streaming })[0];

describe("Markdown document renderer", () => {
  for (const fixture of MARKDOWN_CORPUS) {
    it(`renders ${fixture.id}, prefixes and edits without altering source`, () => {
      const { container, rerender } = render(<ConversationMarkdown block={doc(fixture.source)} />);
      expect(container.querySelector(fixture.selector)).not.toBeNull();
      for (const source of markdownEditSequence(fixture.source)) {
        if (!source) continue;
        rerender(<ConversationMarkdown block={doc(source, true)} />);
        const actual = container.querySelector(".cc-markdown-document")!;
        const fresh = document.createElement("div");
        fresh.innerHTML = renderToStaticMarkup(<ConversationMarkdown block={doc(source, true)} />);
        expect(normalizedDom(actual)).toEqual(normalizedDom(fresh.firstElementChild!));
        expect(conversationRichBlockCopyText(doc(source))).toBe(source);
      }
      rerender(<ConversationMarkdown block={doc(fixture.source)} />);
      expect(container.querySelector(fixture.selector)).not.toBeNull();
    });
  }

  it("keeps root, descendants and visible selection on completion of unchanged source", () => {
    const source = "Selected **text** stays\n\n[link](https://example.test)";
    const { container, rerender } = render(<ConversationRichContent blocks={[doc(source, true)]} richStyle="streaming" />);
    const root = container.querySelector(".cc-markdown-document");
    const paragraph = container.querySelector("p")!;
    const text = paragraph.firstChild!;
    const range = document.createRange();
    range.setStart(text, 0); range.setEnd(text, 8);
    window.getSelection()!.removeAllRanges(); window.getSelection()!.addRange(range);
    rerender(<ConversationRichContent blocks={[doc(source)]} />);
    expect(container.querySelector(".cc-markdown-document")).toBe(root);
    expect(container.querySelector("p")).toBe(paragraph);
    expect(window.getSelection()!.toString()).toBe("Selected");
    expect(paragraph.getAttribute("data-source-start")).toBe("0");
    expect(paragraph.getAttribute("data-source-end")).toBe(String(source.indexOf("\n\n")));
  });

  it("escapes raw HTML and never loads images by default", () => {
    const source = '<script>window.attack=1</script>\n\n<img src="x" onerror="attack()">\n\n![secret](https://tracker.test/pixel)\n\n[bad](javascript:alert%281%29)';
    const { container } = render(<ConversationMarkdown block={doc(source)} />);
    expect(container.querySelector("script,img,a")).toBeNull();
    expect(container.textContent).toContain("<script>");
    expect(container.textContent).toContain("secret");
  });

  it("uses distinct link and image policy and safe external attributes", () => {
    const { container, rerender } = render(<ConversationMarkdown block={doc('[safe](https://example.test) [local](/route) [fragment](#part) ![photo](/photo)')} />);
    expect(screen.getByText("safe").closest("a")?.rel).toBe("noopener noreferrer");
    expect(screen.getByText("local").closest("a")).toBeNull();
    expect(screen.getByText("fragment").closest("a")?.getAttribute("href")).toBe("#part");
    rerender(<ConversationMarkdown block={doc('[local](app:item) ![photo](/photo)')} urlPolicy={{
      resolveLink: (url) => url === "app:item" ? "/authorized/item" : null,
      resolveImage: (url) => url === "/photo" ? "blob:https://console.test/approved" : null,
    }} />);
    expect(container.querySelector("a")?.getAttribute("href")).toBe("/authorized/item");
    expect(container.querySelector("img")?.getAttribute("src")).toBe("blob:https://console.test/approved");
    for (const url of ["javascript:attack()", "data:text/html,attack", "file:///secret", "//evil.test", "java\nscript:attack()"] ) {
      expect(resolveMarkdownLink(url, { resolveLink: () => url })).toBeNull();
      expect(resolveMarkdownImage(url, { resolveImage: () => url })).toBeNull();
    }
    expect(resolveMarkdownLink("unknown:item")).toBeNull();
    expect(resolveMarkdownLink("/relative")).toBeNull();
    expect(resolveMarkdownImage("https://example.test/image")).toBeNull();
  });

  it("opts reusable hosts in through the real shared mapper and forwards URL policy", () => {
    const frames = [{ id: "reply", event: "interaction_complete", data: { result: " [local](/route) ![photo](/photo)\n" }, timestampMs: 1 }];
    const legacy = mapFramesToTimelineEntries(null, frames);
    const entries = mapFramesToTimelineEntries(null, frames, { textMode: "markdown" });
    expect(legacy[0].kind === "message" && legacy[0].blocks?.[0].type).toBe("paragraph");
    expect(conversationEntryText(entries[0])).toBe(" [local](/route) ![photo](/photo)\n");
    const viewState = buildConversationViewState({ memberId: "test", agentLabel: "Test", entries });
    const { container } = render(<ConversationTranscript viewState={viewState} markdownUrlPolicy={{ resolveLink: (url) => url, resolveImage: (url) => url }} />);
    expect(container.querySelector('a[href="/route"]')).not.toBeNull();
    expect(container.querySelector('img[src="/photo"]')).not.toBeNull();
  });

  it("leaves caller-built legacy blocks and typed images on their existing path", () => {
    const { container } = render(<ConversationRichContent blocks={[
      { type: "paragraph", text: "# literal heading" },
      { type: "image", src: "https://example.test/typed.png", mediaType: "image/png", alt: "Typed image" },
    ]} />);
    expect(container.querySelector("h1")).toBeNull();
    expect(container.querySelector("img")?.src).toBe("https://example.test/typed.png");
  });
});

/** Replies shaped like streamed assistant output: headings, loose and tight
 * lists, fences holding blank lines, tables, quotes, an HTML comment, CRLF. */
const STREAMED_REPLIES = [
  ...MARKDOWN_CORPUS.filter((fixture) => fixture.id !== "json").map((fixture) => fixture.source),
  "## Status\n\nHere is a **summary** with `code` and a [link](https://example.test).\n\n- first\n- second\n\n- loose third\n\n  continued paragraph\n\n```ts\nconst a = 1;\n\nconst b = 2;\n```\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n> quoted\n\n> again\n\nDone.",
  "1. one\n2. two\n\n3. three\n\n    indented code\n\nafter\n\n<!-- note\n\nstill a comment -->\n\ntail *text*",
  "# Title\r\n\r\nFirst paragraph.\r\n\r\n~~~\r\nfenced\r\n\r\nmore\r\n~~~\r\n\r\nLast.\r\n",
];

function renderCounts(): Record<string, number> {
  const sink: Record<string, number> = {};
  (globalThis as { __consoleRenderCounts?: Record<string, number> }).__consoleRenderCounts = sink;
  return sink;
}

describe("Streaming Markdown chunks", () => {
  it("splits only where every earlier block is closed, and boundaries survive growth", () => {
    expect(splitMarkdownChunks("a\n\nb\n\nc").map((chunk) => chunk.source)).toEqual(["a\n\n", "b\n\n", "c"]);
    // Fences, list continuations, list items and HTML blocks span blank lines.
    expect(splitMarkdownChunks("```\nx\n\ny\n```\n\nz").map((chunk) => chunk.source)).toEqual(["```\nx\n\ny\n```\n\n", "z"]);
    expect(splitMarkdownChunks("- a\n\n  more\n\n- b\n\nend")).toHaveLength(2);
    expect(splitMarkdownChunks("<!-- a\n\nb -->\n\nc").map((chunk) => chunk.source)).toEqual(["<!-- a\n\nb -->\n\n", "c"]);
    // An open line that may still become a list marker closes nothing yet.
    expect(splitMarkdownChunks("a\n\n-")).toHaveLength(1);
    expect(splitMarkdownChunks("a\n\n1")).toHaveLength(1);
    for (const reply of STREAMED_REPLIES) {
      const full = splitMarkdownChunks(reply);
      expect(full.map((chunk) => chunk.source).join("")).toBe(reply);
      for (let cut = 0; cut <= reply.length; cut += 1) {
        const closed = splitMarkdownChunks(reply.slice(0, cut)).slice(0, -1);
        expect(closed).toEqual(full.slice(0, closed.length));
      }
    }
  });

  it("parses only the open tail per streamed token", () => {
    const reply = STREAMED_REPLIES.at(-3)!;
    const prefix = reply.slice(0, reply.lastIndexOf("Done"));
    const counts = renderCounts();
    try {
      const { rerender } = render(<ConversationMarkdown block={doc(prefix, true)} />);
      counts.MarkdownSourceChars = 0;
      rerender(<ConversationMarkdown block={doc(`${prefix}Do`, true)} />);
      expect(counts.MarkdownSourceChars).toBe("Do".length);
    } finally {
      delete (globalThis as { __consoleRenderCounts?: unknown }).__consoleRenderCounts;
    }
  });

  it("closing a block changes no rendered node: the open tail was already Markdown", () => {
    const { container, rerender } = render(<ConversationMarkdown block={doc("Intro with **bold**\n\n", true)} />);
    const intro = container.querySelector("p")!;
    const bold = intro.querySelector("strong")!;
    rerender(<ConversationMarkdown block={doc("Intro with **bold**\n\nNext", true)} />);
    expect(container.querySelector("p")).toBe(intro);
    expect(intro.querySelector("strong")).toBe(bold);
    expect(container.querySelectorAll("p")).toHaveLength(2);
  });

  for (const [index, reply] of STREAMED_REPLIES.entries()) {
    it(`a streamed reply completes to the DOM of a whole parse (${index})`, () => {
      const { container, rerender } = render(<ConversationMarkdown block={doc(reply.slice(0, 1), true)} />);
      for (let cut = 2; cut <= reply.length; cut += 7) rerender(<ConversationMarkdown block={doc(reply.slice(0, cut), true)} />);
      rerender(<ConversationMarkdown block={doc(reply, true)} />);
      const firstBlock = container.querySelector(".cc-markdown-document")!.firstElementChild;
      rerender(<ConversationMarkdown block={doc(reply)} />);
      // A client render, as static markup through innerHTML folds CRLF.
      const whole = render(<ConversationMarkdown block={doc(reply)} />).container;
      expect(normalizedDom(container.querySelector(".cc-markdown-document")!)).toEqual(normalizedDom(whole.querySelector(".cc-markdown-document")!));
      // Completion keeps the rendered nodes unless definitions resolve across blocks.
      if (!/^ {0,3}\[[^\]\n]+\]:/mu.test(reply)) expect(container.querySelector(".cc-markdown-document")!.firstElementChild).toBe(firstBlock);
    });
  }
});
