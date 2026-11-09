import { Fragment, memo, useMemo, useRef, type ReactNode } from "react";
import ReactMarkdown, { type Components } from "react-markdown";
import remarkGfm from "remark-gfm";

import type { ConversationRichMarkdownBlock } from "@console-core";

/** URL decisions are presentation policy. Resolvers must not fetch or grant access. */
export interface MarkdownUrlPolicy {
  resolveLink?: (url: string) => string | null;
  resolveImage?: (url: string) => string | null;
}

function cleanUrl(url: string): string | null {
  const value = url.trim();
  if (!value || /[\u0000-\u001f\u007f]/u.test(value) || value.startsWith("//")) return null;
  if (/^(?:javascript|vbscript|data|file):/iu.test(value)) return null;
  return value;
}

export function resolveMarkdownLink(url: string, policy?: MarkdownUrlPolicy): string | null {
  const source = cleanUrl(url);
  if (!source) return null;
  const resolved = policy?.resolveLink ? policy.resolveLink(source) : source;
  if (resolved === null) return null;
  const value = cleanUrl(resolved);
  if (!value) return null;
  if (/^(?:https?:\/\/|mailto:|#)/iu.test(value)) return value;
  // Relative and app links require an explicit host resolver. Its output still
  // cannot introduce executable or unrecognized schemes.
  if (policy?.resolveLink && !/^[a-z][a-z\d+.-]*:/iu.test(value)) return value;
  return null;
}

export function resolveMarkdownImage(url: string, policy?: MarkdownUrlPolicy): string | null {
  const source = cleanUrl(url);
  if (!source || !policy?.resolveImage) return null;
  const resolved = policy.resolveImage(source);
  if (resolved === null) return null;
  const value = cleanUrl(resolved);
  if (!value) return null;
  return /^(?:https?:\/\/|blob:)/iu.test(value) || !/^[a-z][a-z\d+.-]*:/iu.test(value) ? value : null;
}

type PositionedNode = { position?: { start: { offset?: number }; end: { offset?: number } } } | undefined;

/** Source offsets of a rendered node; `base` places a chunk within its document. */
function sourcePosition(node: PositionedNode, base = 0) {
  const start = node?.position?.start.offset;
  const end = node?.position?.end.offset;
  return {
    "data-source-start": start === undefined ? undefined : start + base,
    "data-source-end": end === undefined ? undefined : end + base,
  };
}

const plugins = [remarkGfm];

/** A run of whole Markdown blocks within a document source. */
export interface MarkdownChunk {
  start: number;
  source: string;
}

const FENCE = /^ {0,3}(`{3,}|~{3,})/u;
const LIST_MARKER = /^(?:[-+*]|\d{1,9}[.)])(?:[ \t]|$)/u;
const HTML_BLOCK = /^ {0,3}<(?:!--|script\b|pre\b|style\b|textarea\b)/iu;
const HTML_BLOCK_END = /-->|<\/(?:script|pre|style|textarea)>/iu;
/** Link reference and footnote definitions resolve across blocks. */
const DEFINITION = /^ {0,3}\[[^\]\n]+\]:/mu;

/**
 * Split a streaming document into whole blocks that later text cannot
 * change, plus the open tail. A boundary is a blank line outside a fenced
 * code or HTML block that is followed by an unindented line which does not
 * continue a list, so every block before it is closed: parsing the chunks
 * separately yields the same tree as parsing the whole source. Boundaries
 * only move forward as the source grows, so earlier chunks keep their text.
 */
export function splitMarkdownChunks(source: string): MarkdownChunk[] {
  const chunks: MarkdownChunk[] = [];
  let chunkStart = 0;
  let fence: { char: string; length: number } | null = null;
  let html = false;
  let blank = false;
  let lineStart = 0;
  while (lineStart < source.length) {
    const newline = source.indexOf("\n", lineStart);
    const lineEnd = newline === -1 ? source.length : newline;
    const line = source.slice(lineStart, lineEnd).replace(/\r$/u, "");
    if (newline === -1) {
      // The open last line closes the blocks before it once its first
      // character rules out a list marker, fence, HTML block or indentation.
      if (!fence && !html && blank && lineStart > chunkStart && /^[^\s\-+*\d`~<]/u.test(line)) {
        chunks.push({ start: chunkStart, source: source.slice(chunkStart, lineStart) });
        chunkStart = lineStart;
      }
      break;
    }
    if (fence) {
      const close = FENCE.exec(line);
      if (close && close[1][0] === fence.char && close[1].length >= fence.length && line.trim() === close[1]) fence = null;
      blank = false;
    } else if (html) {
      if (HTML_BLOCK_END.test(line)) html = false;
      blank = false;
    } else if (!line.trim()) {
      blank = true;
    } else {
      if (blank && lineStart > chunkStart && !/^\s/u.test(line) && !LIST_MARKER.test(line)) {
        chunks.push({ start: chunkStart, source: source.slice(chunkStart, lineStart) });
        chunkStart = lineStart;
      }
      blank = false;
      const open = FENCE.exec(line);
      if (open) fence = { char: open[1][0], length: open[1].length };
      else if (HTML_BLOCK.test(line) && !HTML_BLOCK_END.test(line)) html = true;
    }
    lineStart = lineEnd + 1;
  }
  chunks.push({ start: chunkStart, source: source.slice(chunkStart) });
  return chunks;
}

/** Whether a streamed document may keep its chunked rendering once complete. */
function chunksMatchWhole(source: string): boolean {
  return !DEFINITION.test(source);
}

/** Test-only accounting of Markdown source parsed, on the render-count sink. */
function countParsedSource(length: number): void {
  const sink = (globalThis as { __consoleRenderCounts?: Record<string, number> }).__consoleRenderCounts;
  if (sink) sink.MarkdownSourceChars = (sink.MarkdownSourceChars ?? 0) + length;
}

function isJsonDocument(source: string): boolean {
  if (!/^\s*[\[{]/u.test(source)) return false;
  try { return typeof JSON.parse(source) === "object"; } catch { return false; }
}

function markdownComponents(urlPolicy: MarkdownUrlPolicy | undefined, base: number): Components {
  return {
    a({ href = "", children, title, node, ...props }) {
      const url = resolveMarkdownLink(href, urlPolicy);
      if (!url) return <span {...sourcePosition(node, base)}>{children}</span>;
      const external = /^https?:\/\//iu.test(url);
      return <a {...props} href={url} title={title} {...sourcePosition(node, base)} {...(external ? { target: "_blank", rel: "noopener noreferrer" } : {})}>{children}</a>;
    },
    img({ src = "", alt = "", title, node }) {
      const url = resolveMarkdownImage(typeof src === "string" ? src : "", urlPolicy);
      return url
        ? <img src={url} alt={alt} title={title} loading="lazy" {...sourcePosition(node, base)} />
        : <span className="cc-markdown-image-placeholder" {...sourcePosition(node, base)}>{alt || "Image"}</span>;
    },
    p({ children, node }) { return <p className="cc-rich-paragraph" {...sourcePosition(node, base)}>{children}</p>; },
    pre({ children, node }) { return <pre className="cc-rich-code-body" {...sourcePosition(node, base)}>{children}</pre>; },
    code({ children, className: codeClass, node }) { return <code className={codeClass} {...sourcePosition(node, base)}>{children}</code>; },
    table({ children, node }) { return <div className="cc-rich-table-wrap"><table className="cc-rich-table" {...sourcePosition(node, base)}>{children}</table></div>; },
    blockquote({ children, node }) { return <blockquote {...sourcePosition(node, base)}>{children}</blockquote>; },
    li({ children, node, className: listClass }) { return <li className={listClass} {...sourcePosition(node, base)}>{children}</li>; },
  };
}

interface MarkdownPartProps {
  source: string;
  base: number;
  urlPolicy?: MarkdownUrlPolicy;
  clobberPrefix: string;
}

/** One parse of `source`; memoised so unchanged chunks are not parsed again. */
const MarkdownPart = memo(function MarkdownPart({ source, base, urlPolicy, clobberPrefix }: MarkdownPartProps) {
  countParsedSource(source.length);
  const components = useMemo(() => markdownComponents(urlPolicy, base), [urlPolicy, base]);
  return <ReactMarkdown remarkPlugins={plugins} remarkRehypeOptions={{ clobberPrefix }} components={components} urlTransform={(url) => url}>{source}</ReactMarkdown>;
});

export interface ConversationMarkdownProps {
  block: ConversationRichMarkdownBlock;
  urlPolicy?: MarkdownUrlPolicy;
  className?: string;
}

export const ConversationMarkdown = memo(function ConversationMarkdown({ block, urlPolicy, className }: ConversationMarkdownProps) {
  // A document that streamed while mounted keeps its chunked rendering after
  // completion when the chunks parse the same as the whole, so completion
  // keeps every rendered node (and any selection inside it).
  const streamed = useRef(false);
  if (block.streaming) streamed.current = true;
  let content: ReactNode;
  if (isJsonDocument(block.source)) {
    content = <pre className="cc-rich-code-body" data-source-start={0} data-source-end={block.source.length}><code className="language-json">{block.source}</code></pre>;
  } else {
    const clobberPrefix = `markdown-${encodeURIComponent(block.id)}-`;
    if (streamed.current && (block.streaming || chunksMatchWhole(block.source))) {
      // Streaming re-parsed the whole reply on every token. Closed blocks now
      // parse once; only the open tail parses again as text arrives. A
      // newline joins chunks as it joins blocks in a whole parse.
      content = splitMarkdownChunks(block.source).map((chunk, index) => (
        <Fragment key={chunk.start}>
          {index > 0 ? "\n" : null}
          <MarkdownPart source={chunk.source} base={chunk.start} urlPolicy={urlPolicy} clobberPrefix={clobberPrefix} />
        </Fragment>
      ));
    } else {
      content = <MarkdownPart source={block.source} base={0} urlPolicy={urlPolicy} clobberPrefix={clobberPrefix} />;
    }
  }
  return (
    <div
      className={["cc-markdown-document", className].filter(Boolean).join(" ")}
      data-markdown-document-id={block.id}
      data-streaming={block.streaming ? "true" : "false"}
    >
      {content}
    </div>
  );
}, (previous, next) => (
  previous.block.id === next.block.id
  && previous.block.source === next.block.source
  && previous.block.streaming === next.block.streaming
  && previous.urlPolicy === next.urlPolicy
  && previous.className === next.className
));
