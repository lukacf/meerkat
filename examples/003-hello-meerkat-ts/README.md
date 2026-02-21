# 003 — Hello Meerkat (TypeScript SDK)

The simplest TypeScript agent. The SDK auto-downloads and spawns `rkat-rpc`
as a child process — no manual binary management needed.

## Prerequisites
```bash
npm install @rkat/sdk   # or: npm link from sdks/typescript
```

## Concepts
- `MeerkatClient` — typed async client
- `connect()` / `close()` — process lifecycle
- `createSession(prompt, options)` — execute a prompt
- `Session` — typed session handle with `.text`, `.usage`, `.id`

## Run
```bash
ANTHROPIC_API_KEY=sk-... npx tsx main.ts
```
