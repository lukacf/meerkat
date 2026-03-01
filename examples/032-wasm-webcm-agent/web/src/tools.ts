/**
 * Shared tool schemas and callback registration for WebCM-backed tools.
 *
 * Both the single-agent (agent.ts) and mob orchestrator (mob.ts) register
 * the same shell / write_file / read_file tools against the WebCM host.
 * This module owns the canonical schemas and the registration helper so
 * the definitions live in exactly one place.
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

// ── Registration helper ─────────────────────────────────────────────────────

/**
 * Clear existing tool callbacks and register the three WebCM tools
 * (shell, write_file, read_file) against the given runtime and VM host.
 */
export function registerWebCMTools(runtime: ToolRuntime, vm: WebCMHost): void {
  runtime.clear_tool_callbacks();

  runtime.register_tool_callback(
    "shell", SHELL_DESCRIPTION, SHELL_SCHEMA,
    async (argsJson: string) => {
      const args = JSON.parse(argsJson);
      const { output, exitCode } = await vm.exec(args.command);
      return JSON.stringify({
        content: exitCode === 0 ? (output || "(no output)") : `Exit code ${exitCode}\n${output}`,
        is_error: exitCode !== 0,
      });
    },
  );

  runtime.register_tool_callback(
    "write_file", WRITE_FILE_DESCRIPTION, WRITE_FILE_SCHEMA,
    async (argsJson: string) => {
      const args = JSON.parse(argsJson);
      await vm.exec(`mkdir -p $(dirname ${args.path})`);
      await vm.writeFile(args.path, args.content);
      return JSON.stringify({ content: `Wrote ${args.path}`, is_error: false });
    },
  );

  runtime.register_tool_callback(
    "read_file", READ_FILE_DESCRIPTION, READ_FILE_SCHEMA,
    async (argsJson: string) => {
      const args = JSON.parse(argsJson);
      try {
        const content = await vm.readFile(args.path);
        return JSON.stringify({ content, is_error: false });
      } catch (e: any) {
        return JSON.stringify({ content: e.message, is_error: true });
      }
    },
  );
}
