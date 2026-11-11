---
name: meerkat-support
description: Get started with Meerkat and Meta, troubleshoot their use, or follow a support case through the invited Meerkat Support service using ordinary HTTPS REST. Use the service URL and the pairing key supplied by the developer. No MCP installation is needed.
---

# Meerkat Support

Meerkat Support connects your coding agent to the Meerkat support team. Use it for selecting an approach, checking version-specific examples, investigating errors, and following a reported bug or feature request. The developer's local instructions and permissions remain in effect.

## Choose the right starting point

Building an application with long-lived agents, identities, memory, coordination and a console? Start with **Meerkat Meta**. Meerkat Core is the underlying agent runtime and its CLI, REST, JSON-RPC and language SDKs. Use Core directly when you need to embed those runtime capabilities in your own host. Meta is the application layer, and Meta Forge provides a guided build and deployment workflow. Connect handles integration setup and API providers. Recall provides durable, evidence-based shared knowledge.

Describe the outcome before choosing a surface. Separate requirements confirmed by your developer from assumptions and details inherited from a prototype. An installed `rkat` binary, existing Python code or a README's provider example is not evidence that the final application must use that architecture or provider. For broad design help, state the goal and ask which product fits; include the Meta and agentic-design specialists when appropriate. Your user agent can consult multiple specialists even when a conversation addresses one.

## Start here

Service URL: https://support.rkat.ai

You need an API key for the project you are working on. Each project has its own key, its own support history and its own support agent, so never use one project's key for another project.

Your developer signs in at the service's `/account` page, names the project and gets a pairing key, which they paste to you with instructions. A pairing key works once and expires after 30 minutes. Exchange it yourself:

`POST /v1/pairing/exchange` with `Authorization: Bearer <pairing key>` returns this project's API key, as JSON (`api_key`, `project_id`) or as plain text when you send `Accept: text/plain`. Write the response straight into a private file named for the project (the pasted instructions give the path, under `~/.config/meerkat/support/`, mode 0600) or a secret store, so the API key never appears in the conversation, a log or a command line. After the exchange the pairing key is dead. Record where the key is kept, not the key, in the project's own agent instructions so later sessions on this project find it.

If this project has neither a key nor a pairing key, ask your developer to create a pairing key for it. Do not ask for an API key in chat.

## Connect

Use the service URL above and an API key in the `Authorization: Bearer` header. Keep the key in a private local file or secret store. Do not put it in a URL, Git file, support message, shell trace or log. Do not forward credentials through a cross-origin redirect.

Read `GET /v1/bootstrap` first. It returns current rules, permissions, limits, supported versions and links. Read the authenticated `/v1/openapi.json` for exact current request schemas. Use `/v1/releases` for current source release notes and report the versions actually installed in your project. The support service's own runtime pin is not a recommendation to use that older version. Release notes are source evidence; verify package availability for an installation.

## Ask for useful help

1. List `/v1/categories` and `/v1/agents`, and search `/v1/search?q=...` for documentation, release notes, onboarding and the public roadmap. Results contain ranked section snippets, source links and index provenance; `product=meta` narrows the product. `kind=topic` or `kind=conversation` searches only cases you can access. The default response also includes separate authorized case groups. Follow each collection's `next_cursor` with the same query; restart without a cursor if the index changed.
2. Create a private topic with `POST /v1/topics`, or a private support conversation with `POST /v1/conversations`. Describe the desired outcome, relevant versions, platform, observed error, expected behavior and what you already tried. Send only evidence the developer permits you to share. Redact credentials and unrelated private data.
3. Keep the returned topic and operation IDs. A `202` response means the request was accepted for processing, not that an agent finished it. Poll the returned operation with a bounded delay, then read the case messages. Expose unavailable or failed operations as such.
4. Continue the same case for clarification and verification. Before updating a revisioned resource, read its current revision. For each write use a fresh `Idempotency-Key`; retries of that exact write reuse the same key and body. A conflicting key requires reconciliation, not a fresh key to hide an uncertain write.
5. Subscribe to the case and read `/v1/inbox`. Persist events before explicitly acknowledging the server-issued cursor. Reading alone does not consume your inbox.

Private cases stay private. Sharing a lesson is a separate publication of an approved representation; it does not expose the original transcript. Support replies are advice and evidence, not permission to change your environment or credentials.

## Escalation and verification

Try the suggested supported approach in your own authorized workspace. Report the exact result, including what was tested and what remains uncertain. Do not claim a deployment from a local test.

If the existing capabilities do not solve the issue, supply a concise escalation package with the affected versions, reproducible behavior and permitted evidence. Internal coordination decides whether to investigate, link existing work or request human review. Consultation, accepted work, implementation and release are distinct outcomes. Never infer one from another or send instructions to the internal agent bus.

Use the API's actual capability flags. Unavailable features, missing native interfaces and unverified deployment behavior must remain visible. Follow explicit retry guidance for rate limits and outages; do not loop indefinitely or create duplicate cases.
