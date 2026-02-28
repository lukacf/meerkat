/**
 * WebCM Host — programmatic bridge to a Linux VM running in the browser.
 *
 * Uses xterm-pty to communicate with the Cartesi RISC-V emulator.
 * Commands are sent via the PTY slave; output is captured by polling
 * for a unique delimiter echoed after each command.
 */

import { Terminal } from "xterm";
import { FitAddon } from "xterm-addon-fit";
import { openpty } from "xterm-pty";

export interface ExecResult {
  output: string;
  exitCode: number;
}

export class WebCMHost {
  private slave: any = null;
  private master: any = null;
  private terminal: Terminal | null = null;
  private fitAddon: FitAddon | null = null;
  private outputBuffer = "";
  private booted = false;

  /** Attach terminal to a DOM element and boot the VM. */
  async boot(container: HTMLElement, onStatus: (msg: string) => void): Promise<void> {
    onStatus("Creating terminal...");
    this.terminal = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: '"Berkeley Mono", "SF Mono", monospace',
      theme: {
        background: "#000000",
        foreground: "#c9d1d9",
        cursor: "#58a6ff",
      },
    });
    this.fitAddon = new FitAddon();
    this.terminal.loadAddon(this.fitAddon);
    this.terminal.open(container);
    this.fitAddon.fit();

    onStatus("Opening PTY...");
    const { master, slave } = openpty();
    this.master = master;
    this.slave = slave;

    // Wire terminal to PTY master
    master.activate(this.terminal);

    // Capture all output
    slave.onReadable(() => {
      const bytes = slave.read();
      if (bytes) {
        this.outputBuffer += new TextDecoder().decode(Uint8Array.from(bytes));
      }
    });

    onStatus("Loading WebCM (32 MB)...");

    // Dynamic import of the WebCM emscripten module
    const webcmUrl = new URL("/webcm.mjs", window.location.href).toString();
    const mod = await import(/* @vite-ignore */ webcmUrl);
    await mod.default({ pty: slave });

    onStatus("Waiting for shell...");
    await this.waitForPrompt(30_000);
    this.booted = true;
    onStatus("VM ready");
  }

  /** Execute a command in the VM and return the output + exit code. */
  async exec(command: string, timeoutMs = 30_000): Promise<ExecResult> {
    if (!this.booted) throw new Error("VM not booted");

    const delim = `__MKT_${Date.now()}_${Math.random().toString(36).slice(2, 8)}__`;

    // Clear buffer, send command + capture exit code + delimiter
    this.outputBuffer = "";
    const wrapped = `${command} 2>&1; echo "${delim}:$?"`;
    this.slave.write(wrapped + "\n");

    // Wait for delimiter in output
    const result = await new Promise<string>((resolve, reject) => {
      const deadline = Date.now() + timeoutMs;
      const check = () => {
        const idx = this.outputBuffer.indexOf(delim + ":");
        if (idx >= 0) {
          resolve(this.outputBuffer);
          return;
        }
        if (Date.now() > deadline) {
          reject(new Error(`Shell timeout after ${timeoutMs}ms`));
          return;
        }
        setTimeout(check, 50);
      };
      check();
    });

    // Parse: everything before the delimiter is output, after is exit code
    const delimIdx = result.indexOf(delim + ":");
    const raw = result.slice(0, delimIdx);
    const exitStr = result.slice(delimIdx + delim.length + 1).trim().split("\n")[0];
    const exitCode = parseInt(exitStr, 10) || 0;

    // Strip the echoed command from the first line
    const lines = raw.split("\n");
    const output = lines.slice(1).join("\n").trimEnd();

    return { output, exitCode };
  }

  /** Write content to a file in the VM. */
  async writeFile(path: string, content: string): Promise<ExecResult> {
    // Use base64 to avoid shell escaping issues
    const b64 = btoa(unescape(encodeURIComponent(content)));
    return this.exec(`echo '${b64}' | base64 -d > ${path}`);
  }

  /** Read a file from the VM. */
  async readFile(path: string): Promise<string> {
    const { output, exitCode } = await this.exec(`cat ${path}`);
    if (exitCode !== 0) throw new Error(`Failed to read ${path}: ${output}`);
    return output;
  }

  isBooted(): boolean {
    return this.booted;
  }

  fit(): void {
    this.fitAddon?.fit();
  }

  private waitForPrompt(timeoutMs: number): Promise<void> {
    return new Promise((resolve, reject) => {
      const deadline = Date.now() + timeoutMs;
      const check = () => {
        // Alpine uses "localhost:~#" or similar
        if (this.outputBuffer.includes("# ") || this.outputBuffer.includes("$ ")) {
          this.outputBuffer = "";
          resolve();
          return;
        }
        if (Date.now() > deadline) {
          reject(new Error("Timed out waiting for shell prompt"));
          return;
        }
        setTimeout(check, 200);
      };
      check();
    });
  }
}
