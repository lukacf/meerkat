/**
 * Tool schemas and callback registration.
 *
 * Two groups:
 * 1. WebCM tools (shell, write_file, read_file) — registered for all agents.
 * 2. Delegation tools (delegate_to_*, check_agent_status) — registered only
 *    for the main agent session (after mob init, before create_session_simple).
 *
 * ORDERING MATTERS: JsToolDispatcher snapshots tool definitions at
 * build_wasm_tool_dispatcher() call time. WebCM tools are registered before
 * init_runtime_from_config (so mob members get them). Delegation tools are
 * registered after mob init but before create_session_simple (so only the
 * main agent gets them).
 */

import type { WebCMHost } from "./webcm-host";

// ── Tool schemas (JSON Schema strings) ──────────────────────────────────────

export const SHELL_SCHEMA = JSON.stringify({
  type: "object",
  properties: {
    command: { type: "string", description: "Shell command to execute in the Alpine Linux VM" },
  },
  required: ["command"],
});

export const WRITE_FILE_SCHEMA = JSON.stringify({
  type: "object",
  properties: {
    path: { type: "string", description: "Absolute path to write to" },
    content: { type: "string", description: "File content" },
  },
  required: ["path", "content"],
});

export const READ_FILE_SCHEMA = JSON.stringify({
  type: "object",
  properties: {
    path: { type: "string", description: "Absolute path to read" },
  },
  required: ["path"],
});

// ── Tool descriptions ───────────────────────────────────────────────────────

export const SHELL_DESCRIPTION = "Execute a shell command in the Alpine Linux VM. Returns stdout+stderr and exit code.";
export const WRITE_FILE_DESCRIPTION = "Write content to a file in the VM filesystem. Creates parent directories if needed.";
export const READ_FILE_DESCRIPTION = "Read the contents of a file from the VM filesystem.";

// ── Runtime interface (subset needed for tool registration) ─────────────────

export interface ToolRuntime {
  register_tool_callback(name: string, description: string, schema_json: string, callback: any): void;
  clear_tool_callbacks(): void;
}

// ── VM command queue (serializes PTY access) ────────────────────────────────

/** Simple async mutex — the PTY can only handle one command at a time. */
let vmQueue: Promise<void> = Promise.resolve();
const noop = () => {};

function serialized<T>(fn: () => Promise<T>): Promise<T> {
  const next = vmQueue.then(fn, fn); // run even if previous rejected
  vmQueue = next.then(noop, noop);   // keep chain alive
  return next;
}

/** Format a tool result for the WASM runtime (content + is_error envelope). */
function toolResult(content: string, isError = false): string {
  return JSON.stringify({ content, is_error: isError });
}

// ── Registration helper ─────────────────────────────────────────────────────

/**
 * Clear existing tool callbacks and register the three WebCM tools
 * (shell, write_file, read_file) against the given runtime and VM host.
 *
 * All VM operations are serialized through a queue because the PTY
 * bridge can only handle one command at a time.
 */
export function registerWebCMTools(runtime: ToolRuntime, vm: WebCMHost): void {
  runtime.clear_tool_callbacks();

  runtime.register_tool_callback(
    "shell", SHELL_DESCRIPTION, SHELL_SCHEMA,
    async (argsJson: string) => {
      const args = JSON.parse(argsJson);
      return serialized(async () => {
        const { output, exitCode } = await vm.exec(args.command);
        return toolResult(
          exitCode === 0 ? (output || "(no output)") : `Exit code ${exitCode}\n${output}`,
          exitCode !== 0,
        );
      });
    },
  );

  runtime.register_tool_callback(
    "write_file", WRITE_FILE_DESCRIPTION, WRITE_FILE_SCHEMA,
    async (argsJson: string) => {
      const args = JSON.parse(argsJson);
      return serialized(async () => {
        await vm.exec(`mkdir -p $(dirname ${args.path})`);
        await vm.writeFile(args.path, args.content);
        return toolResult(`Wrote ${args.path}`);
      });
    },
  );

  runtime.register_tool_callback(
    "read_file", READ_FILE_DESCRIPTION, READ_FILE_SCHEMA,
    async (argsJson: string) => {
      const args = JSON.parse(argsJson);
      return serialized(async () => {
        try {
          const content = await vm.readFile(args.path);
          return toolResult(content);
        } catch (e: any) {
          return toolResult(e.message, true);
        }
      });
    },
  );
}
