/**
 * Minimal agent loop — calls Anthropic API directly from the browser,
 * dispatches tool calls to WebCM, and iterates until done.
 *
 * This is a POC spike. The production version would use meerkat-web-runtime's
 * Rust agent loop with a custom tool dispatcher bridged to WebCM.
 */

import type { WebCMHost } from "./webcm-host";

// ── Tool definitions ────────────────────────────────────────────────────────

const TOOLS = [
  {
    name: "shell",
    description:
      "Execute a shell command in the Alpine Linux VM. Returns stdout+stderr and exit code. Use this for running programs, installing packages (apk add), compiling code, etc.",
    input_schema: {
      type: "object" as const,
      properties: {
        command: {
          type: "string",
          description: "The shell command to execute",
        },
      },
      required: ["command"],
    },
  },
  {
    name: "write_file",
    description:
      "Write content to a file in the VM filesystem. Creates parent directories if needed.",
    input_schema: {
      type: "object" as const,
      properties: {
        path: {
          type: "string",
          description: "Absolute path to write to",
        },
        content: {
          type: "string",
          description: "File content",
        },
      },
      required: ["path", "content"],
    },
  },
  {
    name: "read_file",
    description: "Read the contents of a file from the VM filesystem.",
    input_schema: {
      type: "object" as const,
      properties: {
        path: {
          type: "string",
          description: "Absolute path to read",
        },
      },
      required: ["path"],
    },
  },
];

const SYSTEM_PROMPT = `You are a skilled software engineer with access to an Alpine Linux virtual machine running in the user's browser. You can execute shell commands, read and write files.

The VM has: ash (busybox shell), micropython (use "micropython" not "python3"), lua5.4, quickjs (qjs), tcc (C compiler), git, curl, jq, sqlite3, vim, neovim.

Guidelines:
- Use the shell tool to run commands. Always check exit codes.
- Use write_file to create files (avoids shell escaping issues).
- Use read_file to inspect file contents.
- Install additional packages with: apk add <package>
- Work in /root/ or /tmp/.
- Be concise in your responses. Show your work.`;

// ── Types ───────────────────────────────────────────────────────────────────

interface Message {
  role: "user" | "assistant";
  content: any;
}

interface ToolUse {
  type: "tool_use";
  id: string;
  name: string;
  input: Record<string, any>;
}

interface ToolResult {
  type: "tool_result";
  tool_use_id: string;
  content: string;
  is_error?: boolean;
}

export interface AgentEvent {
  type: "thinking" | "text" | "tool_call" | "tool_result" | "error" | "done";
  content: string;
  toolName?: string;
}

// ── Agent loop ──────────────────────────────────────────────────────────────

export class Agent {
  private apiKey: string;
  private vm: WebCMHost;
  private messages: Message[] = [];
  private onEvent: (e: AgentEvent) => void;

  constructor(apiKey: string, vm: WebCMHost, onEvent: (e: AgentEvent) => void) {
    this.apiKey = apiKey;
    this.vm = vm;
    this.onEvent = onEvent;
  }

  async run(userMessage: string): Promise<void> {
    this.messages.push({ role: "user", content: userMessage });

    // Agent loop: call LLM, execute tools, repeat until no more tool calls
    for (let turn = 0; turn < 20; turn++) {
      const response = await this.callApi();

      if (!response.ok) {
        const err = await response.text();
        this.onEvent({ type: "error", content: `API error: ${response.status} ${err}` });
        return;
      }

      const data = await response.json();
      const assistantContent = data.content;

      // Add assistant message to history
      this.messages.push({ role: "assistant", content: assistantContent });

      // Process response blocks
      const toolUses: ToolUse[] = [];

      for (const block of assistantContent) {
        if (block.type === "text" && block.text) {
          this.onEvent({ type: "text", content: block.text });
        } else if (block.type === "tool_use") {
          toolUses.push(block);
        }
      }

      // If no tool calls, we're done
      if (data.stop_reason === "end_turn" || toolUses.length === 0) {
        this.onEvent({ type: "done", content: "" });
        return;
      }

      // Execute tools and collect results
      const toolResults: ToolResult[] = [];

      for (const tool of toolUses) {
        this.onEvent({
          type: "tool_call",
          content: tool.name === "shell" ? tool.input.command : JSON.stringify(tool.input),
          toolName: tool.name,
        });

        try {
          const result = await this.executeTool(tool);
          toolResults.push({
            type: "tool_result",
            tool_use_id: tool.id,
            content: result,
          });
          this.onEvent({
            type: "tool_result",
            content: result.length > 2000 ? result.slice(0, 2000) + "\n...(truncated)" : result,
            toolName: tool.name,
          });
        } catch (err: any) {
          const errMsg = err.message || String(err);
          toolResults.push({
            type: "tool_result",
            tool_use_id: tool.id,
            content: errMsg,
            is_error: true,
          });
          this.onEvent({ type: "error", content: errMsg });
        }
      }

      // Add tool results as user message and loop
      this.messages.push({ role: "user", content: toolResults });
    }

    this.onEvent({ type: "error", content: "Max turns reached" });
  }

  private async executeTool(tool: ToolUse): Promise<string> {
    switch (tool.name) {
      case "shell": {
        const { output, exitCode } = await this.vm.exec(tool.input.command);
        return exitCode === 0
          ? output || "(no output)"
          : `Exit code ${exitCode}\n${output}`;
      }
      case "write_file": {
        await this.vm.exec(`mkdir -p $(dirname ${tool.input.path})`);
        await this.vm.writeFile(tool.input.path, tool.input.content);
        return `Wrote ${tool.input.path}`;
      }
      case "read_file": {
        return await this.vm.readFile(tool.input.path);
      }
      default:
        throw new Error(`Unknown tool: ${tool.name}`);
    }
  }

  private callApi(): Promise<Response> {
    return fetch("https://api.anthropic.com/v1/messages", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "x-api-key": this.apiKey,
        "anthropic-version": "2023-06-01",
        "anthropic-dangerous-direct-browser-access": "true",
      },
      body: JSON.stringify({
        model: "claude-opus-4-6",
        max_tokens: 4096,
        system: SYSTEM_PROMPT,
        tools: TOOLS,
        messages: this.messages,
      }),
    });
  }
}
