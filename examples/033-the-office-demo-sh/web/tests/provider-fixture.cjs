const assert = require("node:assert/strict");

const ENDPOINT = "https://api.anthropic.com/v1/messages";

/** Complete Anthropic wire responses; never a runtime/mob API replacement. */
function createProviderFixture({ maxRequests = 80 } = {}) {
  assert(Number.isSafeInteger(maxRequests) && maxRequests > 0, "Finite positive request limit required");
  const requests = [];
  const started = Date.now();
  return {
    requests,
    accepts: url => url === ENDPOINT,
    response(request) {
      assert(Date.now() - started < 180000, "Synthetic provider time budget exceeded");
      const responseHeaders = [
        { name: "Access-Control-Allow-Origin", value: "*" },
        { name: "Access-Control-Allow-Methods", value: "POST, OPTIONS" },
        { name: "Access-Control-Allow-Headers", value: "content-type,x-api-key,anthropic-version,anthropic-beta,anthropic-dangerous-direct-browser-access" },
      ];
      if (request.method === "OPTIONS") return { responseCode: 204, responseHeaders };
      assert.equal(request.method, "POST", "Only the Anthropic messages API is synthesized");
      assert(requests.length < maxRequests, "Synthetic provider request budget exceeded");
      const body = JSON.parse(request.postData);
      assert.equal(body.model, "claude-sonnet-4-6");
      assert(Array.isArray(body.messages) && body.messages.length > 0);
      const id = `msg_offline_${requests.length}`;
      const text = JSON.stringify({ headline: "Synthetic offline cycle complete", category: "response" });
      const message = {
        id, type: "message", role: "assistant", model: body.model,
        content: [{ type: "text", text }], stop_reason: "end_turn", stop_sequence: null,
        usage: { input_tokens: 8, output_tokens: 12 },
      };
      requests.push({ id, model: body.model, stream: body.stream === true, messages: body.messages.length });
      const content = body.stream
        ? [
          { type: "message_start", message: { ...message, content: [], stop_reason: null, usage: { input_tokens: 8, output_tokens: 0 } } },
          { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
          { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
          { type: "content_block_stop", index: 0 },
          { type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { output_tokens: 12 } },
          { type: "message_stop" },
        ].map(event => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join("")
        : JSON.stringify(message);
      responseHeaders.push({ name: "Content-Type", value: body.stream ? "text/event-stream" : "application/json" });
      return { responseCode: 200, responseHeaders, body: Buffer.from(content).toString("base64") };
    },
  };
}

module.exports = { createProviderFixture };
