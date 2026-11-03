"use strict";

const crypto = require("node:crypto");

function rpcId(prefix = "rpc") {
  return `${prefix}:${crypto.randomUUID()}`;
}

function normalizeBaseUrl(baseUrl) {
  return String(baseUrl || "").replace(/\/+$/, "");
}

async function fetchJson(baseUrl, route, options = {}) {
  const response = await fetch(`${normalizeBaseUrl(baseUrl)}${route}`, {
    method: options.method || "GET",
    headers: {
      ...(options.headers || {}),
    },
    body: options.body,
    signal: options.signal,
  });

  if (!response.ok) {
    const text = await response.text().catch(() => "");
    throw new Error(`${options.method || "GET"} ${route} failed ${response.status}: ${text}`);
  }

  return response.json();
}

async function jsonRpc(baseUrl, method, params = {}, options = {}) {
  const response = await fetch(`${normalizeBaseUrl(baseUrl)}${options.route || "/console/rpc"}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      ...(options.headers || {}),
    },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: options.id || rpcId(method),
      method,
      params,
    }),
    signal: options.signal,
  });

  if (!response.ok) {
    const text = await response.text().catch(() => "");
    throw new Error(`${method} failed ${response.status}: ${text}`);
  }

  const payload = await response.json();
  if (payload?.error) {
    const details = [];
    if (payload.error.code != null) {
      details.push(`code=${payload.error.code}`);
    }
    if (payload.error.data != null) {
      details.push(`data=${typeof payload.error.data === "string" ? payload.error.data : JSON.stringify(payload.error.data)}`);
    }
    const suffix = details.length ? ` (${details.join(", ")})` : "";
    throw new Error(`${payload.error.message || JSON.stringify(payload.error)}${suffix}`);
  }
  return payload?.result ?? null;
}

function createSseParser(onFrame) {
  let buffer = "";

  function flushBlock(block) {
    const trimmed = block.trim();
    if (!trimmed) {
      return;
    }
    const lines = trimmed.split(/\r?\n/);
    let id = "";
    let event = "message";
    const dataLines = [];

    for (const line of lines) {
      if (line.startsWith(":")) {
        continue;
      }
      if (line.startsWith("id:")) {
        id = line.slice(3).trim();
        continue;
      }
      if (line.startsWith("event:")) {
        event = line.slice(6).trim();
        continue;
      }
      if (line.startsWith("data:")) {
        dataLines.push(line.slice(5).trim());
      }
    }

    const rawData = dataLines.join("\n");
    let data = rawData;
    if (rawData) {
      try {
        data = JSON.parse(rawData);
      } catch {
        data = rawData;
      }
    }

    onFrame({
      id,
      event,
      data,
      rawData,
    });
  }

  return {
    push(chunk) {
      buffer += chunk;
      const parts = buffer.split(/\r?\n\r?\n/);
      buffer = parts.pop() || "";
      for (const part of parts) {
        flushBlock(part);
      }
    },
    end() {
      if (buffer.trim()) {
        flushBlock(buffer);
      }
      buffer = "";
    },
  };
}

async function readSse(response, onFrame) {
  if (!response.ok) {
    const text = await response.text().catch(() => "");
    throw new Error(`SSE request failed ${response.status}: ${text}`);
  }

  if (!response.body) {
    throw new Error("SSE response body missing");
  }

  const parser = createSseParser(onFrame);
  const reader = response.body.getReader();
  const decoder = new TextDecoder();

  while (true) {
    const { value, done } = await reader.read();
    if (done) {
      break;
    }
    parser.push(decoder.decode(value, { stream: true }));
  }

  parser.end();
}

async function openSse(baseUrl, route, options = {}, onFrame) {
  const response = await fetch(`${normalizeBaseUrl(baseUrl)}${route}`, {
    method: options.method || "GET",
    headers: {
      ...(options.headers || {}),
    },
    body: options.body,
    signal: options.signal,
  });
  return readSse(response, onFrame);
}

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
const MACHINE_PEER_TOKEN_RE = /^peer[-_][a-z0-9][a-z0-9_-]*$/i;
const MACHINE_PEER_TOKEN_SUFFIX_RE = /\s+peer[-_][a-z0-9][a-z0-9_-]*$/i;
const EMBEDDED_MACHINE_PEER_TOKEN_RE = /\bpeer[-_][a-z0-9][a-z0-9_-]*\b/gi;
const EMBEDDED_PEER_ACK_TOKEN_RE = /\bACK_?FROM_?PEER_?peer[-_][a-z0-9][a-z0-9_-]*\b/gi;
const EMBEDDED_PEER_RESPONSE_TOKEN_RE = /\bpeer[-_]merge[-_][a-z0-9][a-z0-9_-]*\b/gi;
const LEGACY_INLINE_CODE_PLACEHOLDER_RE = /@@CODE\d+@@/g;

function normalizeProjectDisplayLabel(value) {
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

function normalizeLegacyInlineCodePlaceholders(text) {
  return String(text || "")
    .replace(/\s*@@CODE\d+@@\s*(?:[—–-]\s*)?/g, " ")
    .replace(LEGACY_INLINE_CODE_PLACEHOLDER_RE, " ")
    .replace(/[ \t]{2,}/g, " ")
    .trim();
}

function normalizeEmbeddedMachinePeerTokens(text) {
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
      .replace(/[ \t]{2,}/g, " ")
      .trim())
    .join("\n")
    .trim();
}

function normalizeConversationDisplayLabel(label) {
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

function normalizePeerSteeringPrompt(text) {
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
    return /\bPlease reply with acknowledgement\b/i.test(source)
      ? `Requested an acknowledgement from ${projectLabel} thread.`
      : `Sent a peer message to ${projectLabel} thread.`;
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

function normalizeDisplayPunctuation(text) {
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

function normalizeConversationDisplayText(text) {
  return normalizeDisplayPunctuation(
    normalizePeerSteeringPrompt(normalizeEmbeddedMachinePeerTokens(normalizeLegacyInlineCodePlaceholders(text))),
  );
}

function conversationRichPeerIntentForDisplay(intent, body) {
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

module.exports = {
  conversationRichPeerIntentForDisplay,
  createSseParser,
  fetchJson,
  jsonRpc,
  normalizeBaseUrl,
  normalizeConversationDisplayLabel,
  normalizeConversationDisplayText,
  normalizeProjectDisplayLabel,
  openSse,
  readSse,
  rpcId,
};
