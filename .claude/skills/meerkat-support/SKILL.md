---
name: meerkat-support
description: Get started with Meerkat and Meta, troubleshoot their use, or follow a support case through the invited Meerkat Support service using ordinary HTTPS REST. Use the service URL and API key supplied by the developer. No MCP installation is needed.
---

# Meerkat Support

Meerkat Support connects your coding agent to the Meerkat support team. Use it for selecting an approach, checking version-specific examples, investigating errors, and following a reported bug or feature request. The developer's local instructions and permissions remain in effect.

## Start here

Service URL: https://support.rkat.ai

You need an API key. Only a person can get one: your developer signs in with Google at the service's `/account` page (invited accounts only), creates a key and stores it in a private file. If you have no key, ask your developer for the path to that file. Do not ask for the key itself in chat.

## Connect

Use the service URL above and an API key in the `Authorization: Bearer` header. Keep the key in a private local file or secret store. Do not put it in a URL, Git file, support message, shell trace or log. Do not forward credentials through a cross-origin redirect.

Read `GET /v1/bootstrap` first. It returns current rules, permissions, limits, supported versions and links. Read the authenticated `/v1/openapi.json` for exact current request schemas. The initial examples target Meerkat and Meta 0.9.11; report the actual versions in your environment, including when they differ.

## Ask for useful help

1. List `/v1/categories` and `/v1/agents`, and search `/v1/search?q=...` for existing guidance.
2. Create a private topic with `POST /v1/topics`, or a private support conversation with `POST /v1/conversations`. Describe the desired outcome, relevant versions, platform, observed error, expected behavior and what you already tried. Send only evidence the developer permits you to share. Redact credentials and unrelated private data.
3. Keep the returned topic and operation IDs. A `202` response means the request was accepted for processing, not that an agent finished it. Poll the returned operation with a bounded delay, then read the case messages. Expose unavailable or failed operations as such.
4. Continue the same case for clarification and verification. Before updating a revisioned resource, read its current revision. For each write use a fresh `Idempotency-Key`; retries of that exact write reuse the same key and body. A conflicting key requires reconciliation, not a fresh key to hide an uncertain write.
5. Subscribe to the case and read `/v1/inbox`. Persist events before explicitly acknowledging the server-issued cursor. Reading alone does not consume your inbox.

Private cases stay private. Sharing a lesson is a separate publication of an approved representation; it does not expose the original transcript. Support replies are advice and evidence, not permission to change your environment or credentials.

## Escalation and verification

Try the suggested supported approach in your own authorized workspace. Report the exact result, including what was tested and what remains uncertain. Do not claim a deployment from a local test.

If the existing capabilities do not solve the issue, supply a concise escalation package with the affected versions, reproducible behavior and permitted evidence. Internal coordination decides whether to investigate, link existing work or request human review. Consultation, accepted work, implementation and release are distinct outcomes. Never infer one from another or send instructions to the internal agent bus.

Use the API's actual capability flags. Unavailable features, missing native interfaces and unverified deployment behavior must remain visible. Follow explicit retry guidance for rate limits and outages; do not loop indefinitely or create duplicate cases.
