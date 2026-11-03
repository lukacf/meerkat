import type { ToolCompletionEvidence } from "./tool-completion";
import type { MemberKickoffPhase } from "./member-kickoff";
const SUMMARY_HEADER_RE = /^(\d+)\s+files?\s+changed(?:\s+\+([\d,]+)\s+-([\d,]+))?$/i;
const SUMMARY_FILE_RE = /^(.+?)\s+\+([\d,]+)\s+-([\d,]+)$/;
const FILE_CHANGE_RE = /^(Created|Updated|Modified|Deleted)\b/i;
const TERMINAL_DURATION_RE = /^Worked for\s+.+$/i;
const TERMINAL_STATUS_RE = /^(Success|Running|Failed|Cancelled)$/i;

export type ConversationTableAlignment = "left" | "center" | "right";

export interface ConversationParsedSummaryFile {
  name: string;
  plus: number;
  minus: number;
}

export interface ConversationParsedSummary {
  title: string;
  plus: number;
  minus: number;
  files: ConversationParsedSummaryFile[];
}

/** Raw source for one stable text segment, never inferred runtime state. */
export interface ConversationRichMarkdownBlock {
  type: "markdown";
  id: string;
  source: string;
  streaming: boolean;
}

export type ConversationTextMode = "legacy" | "markdown";

export function buildConversationMarkdownBlocks(
  source: string,
  options: { documentId?: string; streaming?: boolean } = {},
): ConversationRichMarkdownBlock[] {
  if (!source) return [];
  return [{
    type: "markdown",
    id: options.documentId ?? "text",
    source,
    streaming: options.streaming === true,
  }];
}

export interface ConversationRichParagraphBlock {
  type: "paragraph";
  text: string;
  streaming?: boolean;
}

/** Presentation of canonical background-job notice fields, never inferred from prose. */
export interface ConversationRichBackgroundJobBlock {
  type: "background-job";
  jobId: string;
  displayName?: string;
  status: string;
  detail: string;
  copyText: string;
}

/** Presentation of a canonical member-kickoff status notice (`mob.kickoff_<phase>`), never inferred from prose. */
export interface ConversationRichMemberKickoffBlock {
  type: "member-kickoff";
  phase: MemberKickoffPhase;
  member: string;
  role?: string;
  copyText: string;
}

export interface ConversationRichHeadingBlock {
  type: "heading";
  level: number;
  text: string;
}

export interface ConversationRichCodeBlock {
  type: "code";
  language: string;
  body: string;
  highlightedHtml?: string | null;
}

export interface ConversationRichTableBlock {
  type: "table";
  headers: string[];
  alignments: ConversationTableAlignment[];
  rows: string[][];
}

export interface ConversationRichCommandBlock {
  type: "command";
  caption: string;
  title: string;
  body: string;
  output?: string;
  footer?: string;
}

export interface ConversationRichToolCallBlock {
  type: "tool-call";
  toolCallId: string;
  name: string;
  arguments: string;
  result?: string;
  /** Images the tool returned in its result content, in order. */
  images?: ConversationRichImageBlock[];
  status: "pending" | "success" | "error";
  completionEvidence?: ToolCompletionEvidence;
  /** Canonical peer identity for exact authorized display-label lookup. */
  peerIdentity?: string;
  /** Authorized peer display metadata, never a model-supplied tool argument. */
  peerDisplayLabel?: string;
  /** For peer comms tools (send_request, send_message, send_response) */
  peerTarget?: string;
  peerIntent?: string;
  peerBody?: string;
  /** Unmarked blocks retain legacy display summaries; typed owner bodies are verbatim. */
  peerBodyFormat?: "verbatim" | "legacy";
  peerImages?: ConversationRichImageBlock[];
  /** Incoming peer message (received via comms drain, not a tool call) */
  peerIncoming?: boolean;
}

export interface ConversationRichFileChangeBlock {
  type: "file-change";
  verb: string;
  before?: string;
  name: string;
  after?: string;
  plus: number;
  minus: number;
}

export interface ConversationRichDividerBlock {
  type: "divider";
  text: string;
}

export interface ConversationRichThinkingBlock {
  type: "thinking";
  label: string;
  text: string;
  final?: boolean;
  persisted?: boolean;
}

export interface ConversationRichImageBlock {
  type: "image";
  src: string;
  mediaType: string;
  alt?: string;
  width?: number;
  height?: number;
  blobId?: string;
  imageId?: string;
}

export type ConversationRichBlock =
  | ConversationRichMarkdownBlock
  | ConversationRichParagraphBlock
  | ConversationRichBackgroundJobBlock
  | ConversationRichMemberKickoffBlock
  | ConversationRichHeadingBlock
  | ConversationRichCodeBlock
  | ConversationRichTableBlock
  | ConversationRichCommandBlock
  | ConversationRichFileChangeBlock
  | ConversationRichDividerBlock
  | ConversationRichThinkingBlock
  | ConversationRichImageBlock
  | ConversationRichToolCallBlock;

const HIDDEN_PEER_DISPLAY_INTENTS = new Set([
  "completed",
  "complete",
  "queued",
  "queue",
  "steer",
  "checksum_token",
  "peer",
]);

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
// `peer_message`, `peer_request`, and `peer_response` are public Meta
// message-kind names, not opaque peer ids. Keep them available to explanatory
// prose (including inline code) while continuing to hide generated tokens such
// as `peer-merge-123` and `peer_some-runtime-id`.
const MACHINE_PEER_TOKEN_RE = /^peer[-_](?!(?:message|request|response)$)[a-z0-9][a-z0-9_-]*$/i;
// The kind-word exemption must require the token to END after the kind word
// ([-_a-z0-9] continues the token): `\b` alone exempts generated tokens like
// `peer_response-1a2b`, leaking them into prose.
const MACHINE_PEER_TOKEN_SUFFIX_RE = /\s+peer[-_](?!(?:message|request|response)(?![a-z0-9_-]))[a-z0-9][a-z0-9_-]*$/i;
const EMBEDDED_MACHINE_PEER_TOKEN_RE = /\bpeer[-_](?!(?:message|request|response)(?![a-z0-9_-]))[a-z0-9][a-z0-9_-]*\b/gi;
const EMBEDDED_PEER_ACK_TOKEN_RE = /\bACK_?FROM_?PEER_?peer[-_][a-z0-9][a-z0-9_-]*\b/gi;
const EMBEDDED_PEER_RESPONSE_TOKEN_RE = /\bpeer[-_]merge[-_][a-z0-9][a-z0-9_-]*\b/gi;
const LEGACY_INLINE_CODE_PLACEHOLDER_RE = /@@CODE\d+@@/g;

export function normalizeProjectDisplayLabel(value: string | null | undefined): string {
  const text = String(value || "").trim();
  if (!text) {
    return "";
  }
  return text
    .split(/[\s_-]+/u)
    .filter(Boolean)
    .map((part) => part.replace(/^[a-z]/u, (char) => char.toUpperCase()))
    .join(" ");
}

function escapeHtml(value: string): string {
  return String(value || "")
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

export function safeConsoleHref(value: string): string | null {
  const trimmed = String(value || "").trim();
  if (!trimmed) return null;
  if (/[\u0000-\u001f\u007f]/.test(trimmed)) return null;
  const lower = trimmed.toLowerCase();
  if (lower.startsWith("//")) return null;
  if (
    lower.startsWith("http://") ||
    lower.startsWith("https://") ||
    lower.startsWith("mailto:") ||
    lower.startsWith("/") ||
    lower.startsWith("./") ||
    lower.startsWith("../") ||
    lower.startsWith("#")
  ) {
    return trimmed;
  }
  return null;
}

export interface RenderConversationInlineMarkdownOptions {
  // Display normalization rewrites/strips peer-protocol tokens and tidies
  // punctuation for meerkat-studio's conversational surface. Consumers that
  // need faithful rendering of raw agent text (e.g. the Meta console)
  // disable it — matching the parse-side `displayNormalization` option.
  displayNormalization?: boolean;
}

export function renderConversationInlineMarkdown(
  text: string,
  options: RenderConversationInlineMarkdownOptions = {},
): string {
  // Order: code spans first (mask their contents from later passes),
  // then bold (`**x**`), then italic (single `*x*`). Bold must come
  // before italic — otherwise the italic regex would consume one
  // asterisk from each `**` pair.
  const displayNormalization = options.displayNormalization !== false;
  const codeTokens: string[] = [];
  const tokenPrefix = "\uE000CCODE";
  const tokenSuffix = "\uE001";
  const source = displayNormalization
    ? normalizeConversationDisplayText(text || "")
    : String(text || "");
  const escaped = escapeHtml(source)
    .replace(/`([^`]+)`/g, (_match, code) => {
      const index = codeTokens.push(`<code class="cc-rich-inline-code">${code}</code>`) - 1;
      return `${tokenPrefix}${index}${tokenSuffix}`;
    })
    .replace(/\*\*([^*\n]+)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[^*])\*([^*\n]+)\*(?!\*)/g, "$1<em>$2</em>")
    // Underscore emphasis is not allowed intra-word (CommonMark), so
    // identifiers like MEERKAT_TOUR_OK render literally.
    .replace(/(^|[^\w_])_([^_\n]+)_(?![\w_])/g, "$1<em>$2</em>")
    .replace(/\[([^\]]+)\]\(([^)]+)\)/g, (_match, label, href) => {
      const safeHref = safeConsoleHref(href);
      return safeHref
        ? `<a href="${safeHref}" rel="noreferrer">${label}</a>`
        : label;
    })
    .replace(/\n/g, "<br />");

  return escaped
    .replace(new RegExp(`${tokenPrefix}(\\d+)${tokenSuffix}`, "g"), (_match, index) => codeTokens[Number(index)] || "");
}

function normalizeLegacyInlineCodePlaceholders(text: string): string {
  const source = String(text || "");
  if (!LEGACY_INLINE_CODE_PLACEHOLDER_RE.test(source)) {
    return source;
  }

  LEGACY_INLINE_CODE_PLACEHOLDER_RE.lastIndex = 0;
  return source
    .split(/\n/u)
    .map((line) => line
      .replace(/\s*@@CODE\d+@@\s*(?:[—–-]\s*)?/g, " ")
      .replace(/\s*,\s*(?=,|and\b|or\b|[.;:!?]|$)/gi, " ")
      .replace(/\s*\+\s*/g, " ")
      .replace(/\s+([,.;:!?])/g, "$1")
      .replace(/\s{2,}/g, " ")
      .trim())
    .filter(Boolean)
    .join("\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

function normalizeEmbeddedMachinePeerTokens(text: string): string {
  const source = String(text || "");
  if (!EMBEDDED_MACHINE_PEER_TOKEN_RE.test(source) && !EMBEDDED_PEER_ACK_TOKEN_RE.test(source)) {
    return source;
  }

  EMBEDDED_MACHINE_PEER_TOKEN_RE.lastIndex = 0;
  EMBEDDED_PEER_ACK_TOKEN_RE.lastIndex = 0;
  return source
    .split(/\n/u)
    .map((line) => line
      .replace(EMBEDDED_PEER_ACK_TOKEN_RE, "acknowledgement")
      .replace(EMBEDDED_PEER_RESPONSE_TOKEN_RE, "response token")
      .replace(EMBEDDED_MACHINE_PEER_TOKEN_RE, " ")
      .replace(/\bcontaining\s*([.;])/gi, "$1")
      .replace(/^Meta live peer smoke[.:]?\s*/i, "Peer check. ")
      .replace(/\s+([,.;:!?])/g, "$1")
      .replace(/:\s*([.;])/g, "$1")
      .replace(/([.;:!?]){2,}/g, "$1")
      .replace(/\s{2,}/g, " ")
      .trim())
    .filter(Boolean)
    .join("\n")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

function normalizePeerSteeringPrompt(text: string): string {
  const source = String(text || "").trim();
  if (!source) {
    return "";
  }

  if (/^After (?:the peer message|the request) is sent, stop\.?$/i.test(source)) {
    return "";
  }
  if (/^In the request blocks,\s*ask it to send_response\b/i.test(source)) {
    return "";
  }
  const splitSendRequest = source.match(/^Call peers, then send\b.*\bsend_request\b.*?\bto the peered\s+(.+?)\s+thread\b/i);
  if (splitSendRequest) {
    const projectLabel = normalizeProjectDisplayLabel(splitSendRequest[1]) || splitSendRequest[1].trim();
    return `Requested a peer response from ${projectLabel} thread.`;
  }
  const splitExactMessage = source.match(/^Send this exact message body to the peered\s+(.+?)\s+thread\b/i);
  if (splitExactMessage) {
    const projectLabel = normalizeProjectDisplayLabel(splitExactMessage[1]) || splitExactMessage[1].trim();
    if (/\bPlease reply with acknowledgement\b/i.test(source)) {
      return `Requested an acknowledgement from ${projectLabel} thread.`;
    }
    return `Sent a peer message to ${projectLabel} thread.`;
  }

  const standalonePeerInstruction = source.match(/^Use your Meta peer tools only\b[\s\S]*?\bto the peered\s+(.+?)\s+thread\b/i);
  if (standalonePeerInstruction) {
    const projectLabel = normalizeProjectDisplayLabel(standalonePeerInstruction[1]) || standalonePeerInstruction[1].trim();
    const peerLabel = `${projectLabel} thread`;
    if (/\bsend_request\b/i.test(source) || /\bsend_response\b/i.test(source)) {
      return `Requested a peer response from ${peerLabel}.`;
    }
    if (/\bPlease reply with acknowledgement\b/i.test(source)) {
      return `Requested an acknowledgement from ${peerLabel}.`;
    }
    return `Sent a peer message to ${peerLabel}.`;
  }
  if (/^Use your Meta peer tools only\b/i.test(source)) {
    return "";
  }

  const legacyTrustedPeerInstruction = /\bFind your trusted peer\b[\s\S]*?\bsend_message\b[\s\S]*?\bhandling_mode\b/i.test(source);
  const connectedMatch = source.match(
    /^Connected to\s+(.+?)\.\s+(?:Each thread keeps its own transcript and can message the other through Meta|Each thread keeps its own transcript\. They can now message each other)\./is,
  );
  if (connectedMatch && (/\bUse your Meta peer tools only\b/i.test(source) || legacyTrustedPeerInstruction)) {
    const peerLabel = normalizeConversationDisplayLabel(connectedMatch[1]) || connectedMatch[1].trim();
    if (legacyTrustedPeerInstruction) {
      const action = /\bplease reply\b/i.test(source)
        ? `Requested a peer reply from ${peerLabel}.`
        : `Sent a peer message to ${peerLabel}.`;
      return [`Connected to ${peerLabel}.`, action].join("\n");
    }
    if (/\bCall peers, then send\b.*\bsend_request\b/i.test(source) || /\bask it to send_response\b/i.test(source)) {
      return [`Connected to ${peerLabel}.`, `Requested a peer response from ${peerLabel}.`].join("\n");
    }
    if (!/\bSend this exact message body\b/i.test(source)) {
      return source;
    }
    const requestedAcknowledgement = /\bPlease reply with acknowledgement\b/i.test(source);
    const action = requestedAcknowledgement
      ? `Requested an acknowledgement from ${peerLabel}.`
      : `Sent a peer message to ${peerLabel}.`;
    return [`Connected to ${peerLabel}.`, action].join("\n");
  }
  if (legacyTrustedPeerInstruction) {
    return /\bplease reply\b/i.test(source)
      ? "Requested a peer reply."
      : "Sent a peer message.";
  }

  if (/^Call peers, then send_request\b/i.test(source) && /\bAsk the peer to send_response\b/i.test(source)) {
    return "Requested a peer response.";
  }

  return source;
}

function normalizeDisplayPunctuation(text: string): string {
  return String(text || "")
    .split(/\n/u)
    .map((line) => line
      .replace(/\b(verified|received):\s*`?(?:response token|acknowledgement)`?\.?$/i, "$1.")
      .replace(/:\s*\./g, ".")
      .replace(/:\s*$/g, ".")
      .replace(/\s+([,.;:!?])/g, "$1")
      .replace(/([.;:!?]){2,}/g, "$1")
      .trim())
    .filter((line) => line && !/^[\s"'“”‘’`´.,;:!?()[\]{}<>—–-]+$/u.test(line))
    .join("\n")
    .trim();
}

export function normalizeConversationDisplayText(text: string): string {
  return normalizeDisplayPunctuation(
    normalizePeerSteeringPrompt(normalizeEmbeddedMachinePeerTokens(normalizeLegacyInlineCodePlaceholders(text))),
  );
}

export function conversationRichPeerIntentForDisplay(
  intent: string | null | undefined,
  body?: string | null,
): string | undefined {
  const text = String(intent || "").trim();
  if (!text) {
    return undefined;
  }
  if (HIDDEN_PEER_DISPLAY_INTENTS.has(text.toLowerCase()) || UUID_RE.test(text) || MACHINE_PEER_TOKEN_RE.test(text)) {
    return undefined;
  }
  if (body && String(body).trim()) {
    return undefined;
  }
  return text;
}

export function conversationRichPeerTargetForDisplay(target: string | null | undefined): string {
  const text = normalizeConversationDisplayLabel(target);
  if (!text || UUID_RE.test(text)) {
    return "Peer";
  }
  return text;
}

export function normalizeConversationDisplayLabel(label: string | null | undefined): string {
  const text = String(label || "").trim().replace(/\s+/g, " ");
  if (!text || UUID_RE.test(text) || MACHINE_PEER_TOKEN_RE.test(text)) {
    return "";
  }

  const withoutToken = text.replace(MACHINE_PEER_TOKEN_SUFFIX_RE, "").trim();
  if (!withoutToken || UUID_RE.test(withoutToken) || MACHINE_PEER_TOKEN_RE.test(withoutToken)) {
    return "";
  }

  const livePeer = withoutToken.match(/^Peer\s+live\s+(.+)$/i);
  if (livePeer) {
    return `${normalizeProjectDisplayLabel(livePeer[1])} peer thread`;
  }

  return withoutToken
    .replace(/\bpeer\s+(?:source|target)\b/i, "peer thread")
    .replace(/\brequest\s+source\b/i, "request thread")
    .replace(/\bresponse\s+target\b/i, "response thread")
    .replace(/\bmerged\s+request\b/i, "peer request")
    .replace(/\bmerged\s+response\b/i, "peer response")
    .trim();
}

export function conversationRichPeerBodyForDisplay(
  body: string | null | undefined,
  format: "verbatim" | "legacy" = "verbatim",
): string | undefined {
  // Typed owner content is never classified by its spelling. Only callers
  // explicitly rendering the old presentation format use its compatibility rules.
  if (format === "verbatim") return typeof body === "string" && body.trim() ? body : undefined;
  const raw = String(body || "").trim();
  if (!raw) return undefined;
  if (UUID_RE.test(raw) || MACHINE_PEER_TOKEN_RE.test(raw)) return "Response sent.";
  if (/^please\s+send_response\b.*\bresult\.token\b/i.test(raw)) return "Response requested.";
  if (/^please\s+reply\s+with\s+ACK_FROM_PEER_/i.test(raw)) return "Acknowledgement requested.";
  if (/^ACK_?FROM_?PEER_/i.test(raw)) return "Acknowledgement sent.";
  return normalizeConversationDisplayText(raw) || undefined;
}

export function conversationRichBlockHasCopyAction(block: ConversationRichBlock): boolean {
  return block.type === "code" || block.type === "command" || block.type === "file-change";
}

export function conversationRichBlockCopyText(block: ConversationRichBlock): string {
  switch (block.type) {
    case "markdown":
      return block.source;
    case "code":
      return block.body.trim();
    case "command":
      return [block.title, block.body, block.output || "", block.footer || ""].filter(Boolean).join("\n").trim();
    case "file-change":
      return [
        block.verb,
        block.before || "",
        block.name,
        block.after || "",
        `+${block.plus}`,
        `-${block.minus}`,
      ].filter(Boolean).join(" ").replace(/\s+/g, " ").trim();
    case "table":
      return [
        block.headers.join(" | "),
        ...block.rows.map((row) => row.join(" | ")),
      ].join("\n").trim();
    case "background-job":
    case "member-kickoff":
      return block.copyText;
    case "heading":
      return block.text.trim();
    case "paragraph":
    case "divider":
      return block.text.trim();
    case "thinking":
      return [block.label, block.text].filter(Boolean).join("\n").trim();
    case "image":
      return [block.alt || "image", block.blobId || block.src].filter(Boolean).join(" ").trim();
    case "tool-call": {
      if (block.peerTarget) {
        const dir = block.peerIncoming ? "← from" : "→ to";
        const peerBody = conversationRichPeerBodyForDisplay(block.peerBody);
        const images = (block.peerImages || [])
          .map((image) => [image.alt || "image", image.blobId || image.src].filter(Boolean).join(" "))
          .filter(Boolean)
          .join(" ");
        return [
          `${dir} ${block.peerIdentity || block.peerTarget || "Unknown peer"}`,
          conversationRichPeerIntentForDisplay(block.peerIntent, peerBody),
          peerBody,
          images,
          block.result,
        ].filter(Boolean).join(": ").trim();
      }
      const parts = [`$ ${block.name}`];
      if (block.arguments) parts.push(`Input: ${block.arguments}`);
      if (block.result) parts.push(`Result: ${block.result}`);
      return parts.join("\n");
    }
    default:
      return "";
  }
}

export function conversationRichBlocksToText(blocks: ConversationRichBlock[] | null | undefined): string {
  const text = (blocks || [])
    .map((block) => conversationRichBlockCopyText(block))
    .filter(Boolean)
    .join("\n\n");
  return blocks?.some((block) => block.type === "markdown" || block.type === "tool-call") ? text : text.trim();
}

export function parseStreamingConversationRichBlocks(
  content: string,
  options?: ConversationRichParseOptions,
): ConversationRichBlock[] {
  const source = String(content || "").trimEnd();
  if (!source.trim()) {
    return [];
  }

  const stableEnd = streamingStablePrefixLength(source);
  const stable = stableEnd > 0 ? source.slice(0, stableEnd).trim() : "";
  const tail = source.slice(stableEnd).trim();
  const blocks = stable ? parseConversationRichBlocks(stable, options) : [];

  if (tail) {
    const tailText = tail.replace(/\n{3,}/g, "\n\n");
    // An unclosed fence tail is code-in-flight: keep it verbatim instead of
    // running the inline-marker hider over its backticks.
    const visibleTail = (unclosedFenceStartIndex(tailText) !== null
      ? tailText
      : hideIncompleteInlineTail(tailText)).trim();
    if (visibleTail) {
      blocks.push({ type: "paragraph", text: visibleTail, streaming: true });
    }
  }

  return compactConversationBlocks(blocks);
}

function streamingStablePrefixLength(source: string): number {
  const fenceStart = unclosedFenceStartIndex(source);
  const scanEnd = fenceStart ?? source.length;
  const scanSource = source.slice(0, scanEnd);
  let stableEnd = 0;
  const blankLineRe = /\n[ \t]*\n/gu;
  let match: RegExpExecArray | null;
  while ((match = blankLineRe.exec(scanSource))) {
    stableEnd = blankLineRe.lastIndex;
  }
  return stableEnd;
}

function hideIncompleteInlineTail(source: string): string {
  const firstOpen = firstUnclosedInlineMarkerIndex(source);
  if (firstOpen === null) {
    return source;
  }
  return source.slice(0, firstOpen).replace(/\s+$/u, "");
}

function firstUnclosedInlineMarkerIndex(source: string): number | null {
  return minNullable([
    unclosedDelimitedMarkerIndex(source, "`"),
    unclosedDelimitedMarkerIndex(source, "**"),
    unclosedDelimitedMarkerIndex(source, "*"),
    unclosedDelimitedMarkerIndex(source, "_"),
    unclosedLinkStartIndex(source),
  ]);
}

function minNullable(values: Array<number | null>): number | null {
  return values.reduce<number | null>((min, value) => {
    if (value === null) return min;
    return min === null || value < min ? value : min;
  }, null);
}

function unclosedDelimitedMarkerIndex(source: string, delimiter: "`" | "*" | "**" | "_"): number | null {
  const positions: number[] = [];
  for (let index = 0; index < source.length; index++) {
    if (source[index - 1] === "\\") continue;
    if (delimiter === "**") {
      if (source.slice(index, index + 2) !== "**") continue;
      positions.push(index);
      index += 1;
      continue;
    }
    const char = source[index];
    if (char !== delimiter) continue;
    if (delimiter === "*" && (source[index - 1] === "*" || source[index + 1] === "*")) continue;
    if (delimiter === "_" && isAlphaNumeric(source[index - 1]) && isAlphaNumeric(source[index + 1])) continue;
    if ((delimiter === "*" || delimiter === "_") && isLineBulletMarker(source, index)) continue;
    positions.push(index);
  }
  return positions.length % 2 === 1 ? positions.at(-1) ?? null : null;
}

function unclosedLinkStartIndex(source: string): number | null {
  const match = source.match(/\[[^\]\n]*\]\([^)\n]*$/u);
  return match?.index ?? null;
}

function isAlphaNumeric(value: string | undefined): boolean {
  return Boolean(value && /[A-Za-z0-9]/u.test(value));
}

function isLineBulletMarker(source: string, index: number): boolean {
  const before = source.slice(0, index);
  const linePrefix = before.slice(before.lastIndexOf("\n") + 1);
  return linePrefix.trim().length === 0 && /\s/u.test(source[index + 1] || "");
}

function unclosedFenceStartIndex(source: string): number | null {
  const fenceRe = /^```/gmu;
  let match: RegExpExecArray | null;
  let openStart: number | null = null;
  while ((match = fenceRe.exec(source))) {
    openStart = openStart === null ? match.index : null;
  }
  return openStart;
}

export function parseConversationSummary(content: string): ConversationParsedSummary | null {
  const lines = String(content || "")
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);

  if (lines.length < 2) {
    return null;
  }

  const headerMatch = lines[0].match(SUMMARY_HEADER_RE);
  if (!headerMatch) {
    return null;
  }

  const files: ConversationParsedSummaryFile[] = [];
  for (const line of lines.slice(1)) {
    const fileMatch = line.match(SUMMARY_FILE_RE);
    if (!fileMatch) {
      break;
    }
    files.push({
      name: fileMatch[1].trim(),
      plus: Number.parseInt(fileMatch[2].replaceAll(",", ""), 10) || 0,
      minus: Number.parseInt(fileMatch[3].replaceAll(",", ""), 10) || 0,
    });
  }

  if (files.length === 0) {
    return null;
  }

  return {
    title: lines[0].replace(/\s+\+[\d,]+\s+-[\d,]+$/u, ""),
    plus: Number.parseInt((headerMatch[2] || "0").replaceAll(",", ""), 10) || files.reduce((sum, file) => sum + file.plus, 0),
    minus: Number.parseInt((headerMatch[3] || "0").replaceAll(",", ""), 10) || files.reduce((sum, file) => sum + file.minus, 0),
    files,
  };
}

/// Heuristic JSON detector + parser. Returns the parsed value if the
/// trimmed string starts with `{`/`[` and ends with the matching brace
/// AND parses as JSON. Schema-agnostic — we don't care what the JSON
/// is, only that it shouldn't be re-flowed through the markdown
/// inline regexes (which would italicise stray `*` / `_` characters
/// inside string values and produce a wall of mush).
function tryParseJson(source: string): unknown | null {
  const trimmed = source.trim();
  if (trimmed.length < 2) return null;
  const first = trimmed[0];
  const last = trimmed[trimmed.length - 1];
  const looksObj = first === "{" && last === "}";
  const looksArr = first === "[" && last === "]";
  if (!looksObj && !looksArr) return null;
  try {
    return JSON.parse(trimmed);
  } catch {
    return null;
  }
}

export interface ConversationRichParseOptions {
  /**
   * Apply the display-text normalization layer (legacy placeholder cleanup,
   * machine peer-token scrubbing, steering-prompt rewrites, punctuation
   * tidying) before parsing. Defaults to true — the meerkat-studio desktop
   * transcript relies on it. Consumers that do their own envelope handling
   * on the parsed output (e.g. the meta console adapters) should pass
   * `{ displayNormalization: false }` to parse the text faithfully.
   */
  displayNormalization?: boolean;
}

export function parseConversationRichBlocks(
  content: string,
  options?: ConversationRichParseOptions,
): ConversationRichBlock[] {
  const displayNormalization = options?.displayNormalization !== false;
  const source = String(content || "").trim();
  if (!source) {
    return [];
  }

  // Whole-message JSON: structured-output extraction (e.g. a Fugue
  // gate-schema reviewer) ships the agent's run result as a JSON
  // string verbatim. Render as a code block instead of running it
  // through the prose / markdown pipeline. Mid-message JSON is also
  // handled per-section in `parseConversationTextBlocks`.
  const wholeJson = tryParseJson(source);
  if (wholeJson !== null) {
    return [{
      type: "code",
      language: "json",
      body: JSON.stringify(wholeJson, null, 2),
    }];
  }

  const blocks: ConversationRichBlock[] = [];
  const fenceRe = /```([^\n`]*)\n([\s\S]*?)```/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;

  while ((match = fenceRe.exec(source))) {
    const before = source.slice(lastIndex, match.index);
    blocks.push(...parseConversationTextBlocks(before, displayNormalization));
    blocks.push({
      type: "code",
      language: (match[1] || "text").trim() || "text",
      body: match[2].replace(/\n+$/u, ""),
    });
    lastIndex = fenceRe.lastIndex;
  }

  blocks.push(...parseConversationTextBlocks(source.slice(lastIndex), displayNormalization));
  return compactConversationBlocks(blocks);
}

// Display normalization intentionally removes empty lines for compact labels
// and protocol chatter. Rich-content parsing cannot lose those boundaries:
// the streaming parser has already promoted text before the last blank line
// into stable blocks, so collapsing the same break at finalization changes the
// block topology and destroys the live tail node. Protect paragraph breaks
// while the full-message normalizers run, then restore them before splitting.
const PARAGRAPH_BREAK_SENTINEL = "\uE000CCPARAGRAPHBREAK\uE001";
const PARAGRAPH_BREAK_SENTINEL_ESCAPED = PARAGRAPH_BREAK_SENTINEL.replace(
  /[.*+?^${}()|[\]\\]/gu,
  "\\$&",
);
// Restore matches RUNS of sentinels with any whitespace/newlines between:
// the display normalizers can delete an entire line separating two breaks
// (a markdown `---` rule, a punctuation-only line, a machine-token-only
// line), leaving sentinels adjacent — a newline-anchored single-sentinel
// pattern consumes the shared newline on the first match and leaks the
// literal sentinel text into the rendered paragraph.
const PARAGRAPH_BREAK_SENTINEL_RUN = new RegExp(
  `[ \\t]*(?:${PARAGRAPH_BREAK_SENTINEL_ESCAPED}[ \\t]*(?:\\r?\\n)?[ \\t]*)+`,
  "gu",
);

function normalizeConversationTextForParsing(fragment: string): string {
  // Content that literally contains the private-use sentinel (adversarial or
  // binary-ish text) must not forge paragraph breaks or corrupt restoration.
  const raw = String(fragment || "").replace(PARAGRAPH_BREAK_SENTINEL_RUN, " ");
  const protectedFragment = raw.replace(
    /\r?\n[ \t]*\r?\n(?:[ \t]*\r?\n)*/gu,
    `\n${PARAGRAPH_BREAK_SENTINEL}\n`,
  );
  return normalizeConversationDisplayText(protectedFragment).replace(
    PARAGRAPH_BREAK_SENTINEL_RUN,
    "\n\n",
  );
}

function parseConversationTextBlocks(fragment: string, displayNormalization = true): ConversationRichBlock[] {
  const source = (displayNormalization
    ? normalizeConversationTextForParsing(fragment)
    : String(fragment || "")).trim();
  if (!source) {
    return [];
  }

  const sections = source
    .split(/\n{2,}/u)
    .map((section) => section.trim())
    .filter(Boolean);
  const blocks: ConversationRichBlock[] = [];

  for (const section of sections) {
    // Per-section JSON: the section is one paragraph (split on `\n\n`).
    // A section that parses as JSON renders as a code block rather
    // than getting fed through the markdown inline regexes.
    const sectionJson = tryParseJson(section);
    if (sectionJson !== null) {
      blocks.push({
        type: "code",
        language: "json",
        body: JSON.stringify(sectionJson, null, 2),
      });
      continue;
    }

    // Headings and pipe tables routinely arrive glued to prose with single
    // newlines (one "section"); the line-wise splitter handles those mixed
    // sections and subsumes the pure whole-section heading/table cases.
    const mixed = splitMixedProseSection(section);
    if (mixed) {
      blocks.push(...mixed);
      continue;
    }

    const fileChange = parseConversationFileChangeBlock(section);
    if (fileChange) {
      blocks.push(fileChange);
      continue;
    }

    const command = parseConversationCommandBlock(section);
    if (command) {
      blocks.push(command);
      continue;
    }

    if (TERMINAL_DURATION_RE.test(section)) {
      blocks.push({ type: "divider", text: section });
      continue;
    }

    const normalized = section
      .replace(/^\s*[-*]\s+/gm, "")
      .replace(/\n{2,}/g, "\n")
      .trim();

    if (normalized) {
      blocks.push({ type: "paragraph", text: normalized });
    }
  }

  return blocks;
}

function compactConversationBlocks(blocks: ConversationRichBlock[]): ConversationRichBlock[] {
  const deduped: ConversationRichBlock[] = [];
  for (const block of blocks) {
    const previous = deduped.at(-1);
    if (
      block.type === "paragraph"
      && previous?.type === "file-change"
      && previous.name
      && block.text.startsWith(previous.name)
    ) {
      continue;
    }
    deduped.push(block);
  }
  return deduped;
}

// Split a single \n-separated section into heading / table / prose blocks.
// Returns null when the section contains no structural markdown, so callers
// fall through to the legacy per-section handling.
function splitMixedProseSection(section: string): ConversationRichBlock[] | null {
  const lines = String(section || "").split(/\n/u);
  const blocks: ConversationRichBlock[] = [];
  let prose: string[] = [];
  let structural = false;

  const flushProse = () => {
    const text = prose
      .join("\n")
      .replace(/^(\s*)[-*]\s+/gm, "$1\u2022 ")
      .trim();
    prose = [];
    if (text) {
      blocks.push({ type: "paragraph", text });
    }
  };

  for (let index = 0; index < lines.length; index += 1) {
    const trimmed = lines[index].trim();

    const headingMatch = trimmed.match(/^(#{1,6})\s+(.+)$/u);
    if (headingMatch) {
      flushProse();
      blocks.push({
        type: "heading",
        level: headingMatch[1].length,
        text: headingMatch[2].trim(),
      });
      structural = true;
      continue;
    }

    if (trimmed.startsWith("|")) {
      const next = (lines[index + 1] || "").trim();
      const nextAlignment = next.startsWith("|") || /^[\s:|-]+$/u.test(next)
        ? parseTableAlignment(splitMarkdownTableRow(next))
        : null;
      if (nextAlignment && nextAlignment.length) {
        let end = index + 2;
        while (end < lines.length && lines[end].trim().startsWith("|")) {
          end += 1;
        }
        const table = parseConversationTableBlock(lines.slice(index, end).join("\n"));
        if (table) {
          flushProse();
          blocks.push(table);
          structural = true;
          index = end - 1;
          continue;
        }
      }
    }

    prose.push(lines[index]);
  }

  if (!structural) {
    return null;
  }
  flushProse();
  return blocks;
}

function parseConversationHeadingBlock(section: string): ConversationRichBlock[] | null {
  const lines = String(section || "")
    .split(/\n/u)
    .map((line) => line.trim())
    .filter(Boolean);
  if (!lines.length || !lines[0].startsWith("#")) {
    return null;
  }

  const headingMatch = lines[0].match(/^(#{1,6})\s+(.+)$/u);
  if (!headingMatch) {
    return null;
  }

  const blocks: ConversationRichBlock[] = [{
    type: "heading",
    level: headingMatch[1].length,
    text: headingMatch[2].trim(),
  }];
  const rest = lines.slice(1).join("\n").trim();
  if (rest) {
    blocks.push({ type: "paragraph", text: rest });
  }
  return blocks;
}

function splitMarkdownTableRow(line: string): string[] {
  const source = String(line || "")
    .trim()
    .replace(/^\|/u, "")
    .replace(/\|$/u, "");

  const cells: string[] = [];
  let current = "";
  let escaping = false;
  let codeFenceDepth = 0;

  for (const character of source) {
    if (escaping) {
      current += character;
      escaping = false;
      continue;
    }
    if (character === "\\") {
      escaping = true;
      continue;
    }
    if (character === "`") {
      codeFenceDepth = codeFenceDepth === 0 ? 1 : 0;
      current += character;
      continue;
    }
    if (character === "|" && codeFenceDepth === 0) {
      cells.push(current.trim());
      current = "";
      continue;
    }
    current += character;
  }

  cells.push(current.trim());
  return cells;
}

function parseTableAlignment(cells: string[]): ConversationTableAlignment[] | null {
  if (!cells.length || !cells.every((cell) => /^:?-{3,}:?$/u.test(cell))) {
    return null;
  }

  return cells.map((cell) => {
    const trimmed = cell.trim();
    if (trimmed.startsWith(":") && trimmed.endsWith(":")) {
      return "center";
    }
    if (trimmed.endsWith(":")) {
      return "right";
    }
    return "left";
  });
}

function parseConversationTableBlock(section: string): ConversationRichTableBlock | null {
  const lines = String(section || "")
    .split(/\n/u)
    .map((line) => line.trim())
    .filter(Boolean);

  if (lines.length < 2) {
    return null;
  }

  const headers = splitMarkdownTableRow(lines[0]);
  const alignments = parseTableAlignment(splitMarkdownTableRow(lines[1]));
  if (!headers.length || !alignments || headers.length !== alignments.length) {
    return null;
  }

  const rows = lines
    .slice(2)
    .map((line) => splitMarkdownTableRow(line))
    .filter((cells) => cells.length > 0 && cells.some((cell) => cell.length > 0))
    .map((cells) => headers.map((_header, index) => cells[index] || ""));

  return {
    type: "table",
    headers,
    alignments,
    rows,
  };
}

function parseConversationFileChangeBlock(section: string): ConversationRichFileChangeBlock | null {
  const compact = String(section || "").replace(/\s*\n\s*/g, " ").trim();
  if (!compact) {
    return null;
  }

  const header = compact.match(FILE_CHANGE_RE);
  if (!header) {
    return null;
  }

  const verb = header[1];
  const statsMatch = compact.match(/\s+\+([\d,]+)\s+-([\d,]+)\s*$/u);
  const plus = Number.parseInt((statsMatch?.[1] || "1").replaceAll(",", ""), 10) || 0;
  const minus = Number.parseInt((statsMatch?.[2] || "0").replaceAll(",", ""), 10) || 0;
  const body = statsMatch ? compact.slice(0, statsMatch.index).trim() : compact;
  const fileMatches = [...body.matchAll(/`([^`]+)`/gu)];
  const fileMatch = fileMatches.find((candidate) => !candidate[1].includes("/")) || fileMatches[0];
  if (!fileMatch) {
    return null;
  }

  const fileToken = fileMatch[0];
  const fileName = fileMatch[1].trim();
  const bodyAfterVerb = body.slice(verb.length).trim();
  const tokenIndex = bodyAfterVerb.indexOf(fileToken);
  const before = tokenIndex >= 0 ? bodyAfterVerb.slice(0, tokenIndex).trim() : "";
  const after = tokenIndex >= 0
    ? bodyAfterVerb.slice(tokenIndex + fileToken.length).trim()
    : bodyAfterVerb.replace(fileToken, "").trim();

  return {
    type: "file-change",
    verb,
    before,
    name: fileName,
    after,
    plus,
    minus,
  };
}

function parseConversationCommandBlock(section: string): ConversationRichCommandBlock | null {
  const lines = String(section || "")
    .split(/\n/u)
    .map((line) => line.replace(/\s+$/u, ""))
    .filter((line) => line.trim().length > 0);
  if (!lines.length) {
    return null;
  }

  const commandIndex = lines.findIndex((line) => line.trim().startsWith("$ "));
  if (commandIndex === -1) {
    return null;
  }

  const command = lines[commandIndex].trim();
  const prefix = lines.slice(0, commandIndex).filter(Boolean);
  const footerCandidate = lines.at(-1)?.trim() || "";
  const footer = TERMINAL_STATUS_RE.test(footerCandidate) ? footerCandidate : "";
  const outputStart = commandIndex + 1;
  const outputEnd = footer ? lines.length - 1 : lines.length;
  const output = lines.slice(outputStart, outputEnd).join("\n").trim();

  return {
    type: "command",
    caption: prefix[0] || "Ran command",
    title: prefix[1] || "Shell",
    body: command,
    output,
    footer,
  };
}
