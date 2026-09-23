/** Successful synthetic Anthropic responses; never forwards a provider request. */
export function anthropicReply(request, sequence, text) {
  const message = {
    id: `synthetic-${sequence}`, type: "message", role: "assistant", model: request.model,
    content: [{ type: "text", text }], stop_reason: "end_turn", stop_sequence: null,
    usage: { input_tokens: 10, output_tokens: 5 },
  };
  if (!request.stream) return { contentType: "application/json", body: JSON.stringify(message) };
  const events = [
    { type: "message_start", message: { ...message, content: [], stop_reason: null, usage: { input_tokens: 10, output_tokens: 0 } } },
    { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { output_tokens: 5 } },
    { type: "message_stop" },
  ];
  return { contentType: "text/event-stream", body: events.map(event => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join("") };
}
