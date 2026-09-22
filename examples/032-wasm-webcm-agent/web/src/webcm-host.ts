/**
 * WebCM Host — programmatic bridge to a Linux VM running in the browser.
 *
 * Uses xterm-pty to communicate with the Cartesi RISC-V emulator.
 * VM output is captured via master.onWrite (data flowing from VM to terminal).
 * Commands use unique delimiters to detect completion and extract output.
 */

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { openpty, type TermiosConfig } from "xterm-pty";

export interface ExecResult {
  output: string;
  exitCode: number;
}

export function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

export function rawTermios(termios: TermiosConfig): TermiosConfig {
  return {
    ...termios,
    iflag: termios.iflag & ~0x5eb,
    cflag: (termios.cflag & ~0x130) | 0x30,
    lflag: termios.lflag & ~0x804b,
    oflag: termios.oflag | 0x1,
  };
}

export function frameCommand(command: string, marker: string): string {
  // The whole group is parsed before its start marker is emitted. A trailing
  // comment cannot consume the closing brace or the completion marker.
  return `{ printf '\\036${marker}:START\\037';\n${command}\n} 2>&1; printf '\\036${marker}:END:%s\\037' \"$?\"\n`;
}

export function parseCommandOutput(buffer: string, marker: string): ExecResult | null {
  const start = `\x1e${marker}:START\x1f`;
  const from = buffer.indexOf(start);
  if (from < 0) return null;
  const rest = buffer.slice(from + start.length);
  const end = new RegExp(`\\x1e${marker}:END:(\\d+)\\x1f`).exec(rest);
  if (!end) return null;
  return { output: rest.slice(0, end.index).replaceAll("\r\n", "\n"), exitCode: Number(end[1]) };
}

export class WebCMHost {
  private slave: any = null;
  private terminal: Terminal | null = null;
  private fitAddon: FitAddon | null = null;
  private outputBuffer = "";
  private booted = false;
  private bootPromise: Promise<void> | null = null;
  private outputListener: { dispose(): void } | null = null;

  /** Attach terminal to a DOM element and boot the VM. */
  boot(container: HTMLElement, onStatus: (msg: string) => void): Promise<void> {
    if (this.booted) return Promise.resolve();
    if (this.bootPromise) return this.bootPromise;
    this.bootPromise = this.bootOnce(container, onStatus).catch(error => {
      this.outputListener?.dispose();
      this.outputListener = null;
      this.terminal?.dispose();
      this.terminal = null;
      this.fitAddon = null;
      this.slave = null;
      this.booted = false;
      this.outputBuffer = "";
      throw error;
    }).finally(() => { this.bootPromise = null; });
    return this.bootPromise;
  }

  private async bootOnce(container: HTMLElement, onStatus: (msg: string) => void): Promise<void> {
    onStatus("Creating terminal...");
    this.terminal = new Terminal({
      cursorBlink: true,
      fontSize: 13,
      fontFamily: '"Berkeley Mono", "SF Mono", monospace',
      fontWeight: 400,
      scrollback: 5000,
      theme: {
        background: "#000000",
        foreground: "#c9d1d9",
        cursor: "#58a6ff",
      },
    });
    this.terminal.open(container);
    this.terminal.focus();

    onStatus("Opening PTY...");
    const { master, slave } = openpty();
    this.slave = slave;

    // Configure terminal for raw mode (required by WebCM).
    // Disables echo, canonical mode, signals; enables CS8 and OPOST for clean I/O.
    slave.ioctl("TCSETS", rawTermios(slave.ioctl("TCGETS")));

    // Capture VM output via master.onWrite. This event fires with
    // [Uint8Array, callback] for every chunk the VM writes to stdout.
    // We MUST subscribe BEFORE loadAddon, because activate() also
    // subscribes — our listener runs alongside the terminal writer.
    const decoder = new TextDecoder();
    this.outputListener = master.onWrite(([data, _cb]: [Uint8Array, () => void]) => {
      this.outputBuffer += decoder.decode(data, { stream: true });
    });
    // Connect master to xterm.js as addon
    this.terminal.loadAddon(master);

    // Fit after master is loaded
    this.fitAddon = new FitAddon();
    this.terminal.loadAddon(this.fitAddon);
    this.fitAddon.fit();

    onStatus("Loading WebCM (~30 MB)...");

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
  async exec(command: string, timeoutMs = 60_000): Promise<ExecResult> {
    if (!this.booted) throw new Error("VM not booted");

    const delim = `__MKT_${Date.now()}_${Math.random().toString(36).slice(2, 8)}__`;

    this.outputBuffer = "";
    this.writeToShell(frameCommand(command, delim));
    return new Promise<ExecResult>((resolve, reject) => {
      const deadline = Date.now() + timeoutMs;
      const check = () => {
        const result = parseCommandOutput(this.outputBuffer, delim);
        if (result) {
          resolve(result);
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

  }

  /** Write content to a file in the VM. */
  async writeFile(path: string, content: string): Promise<ExecResult> {
    // Base64 encode and split into chunks to avoid PTY buffer overflow.
    // terminal.paste() processes each character through the line discipline,
    // so very long single-line commands time out.
    const b64 = btoa(unescape(encodeURIComponent(content)));
    const CHUNK = 512;
    if (b64.length <= CHUNK) {
      return this.exec(`printf '%s' '${b64}' | base64 -d > ${shellQuote(path)}`);
    }
    // Multi-chunk: write base64 to a temp file in chunks, then decode
    const tmp = `${path}.mkt-${crypto.randomUUID()}.b64`;
    const quoted = shellQuote(tmp);
    let result = await this.exec(`true > ${quoted}`);
    if (result.exitCode !== 0) return result;
    try {
      for (let i = 0; i < b64.length; i += CHUNK) {
        const chunk = b64.slice(i, i + CHUNK);
        result = await this.exec(`printf '%s' '${chunk}' >> ${quoted}`);
        if (result.exitCode !== 0) return result;
      }
      return await this.exec(`base64 -d < ${quoted} > ${shellQuote(path)}`);
    } finally {
      await this.exec(`rm -f -- ${quoted}`);
    }
  }

  /** Read a file from the VM. */
  async readFile(path: string): Promise<string> {
    // Transport file bytes as base64 so PTY newline processing is lossless.
    const { output, exitCode } = await this.exec(`base64 < ${shellQuote(path)}`);
    if (exitCode !== 0) throw new Error(`Failed to read ${path}: ${output}`);
    return new TextDecoder().decode(Uint8Array.from(atob(output.replace(/\s/g, "")), c => c.charCodeAt(0)));
  }

  /** Debug: get raw output buffer contents. */
  getOutputBuffer(): string {
    return this.outputBuffer;
  }

  isBooted(): boolean {
    return this.booted;
  }

  fit(): void {
    this.fitAddon?.fit();
  }

  /** Send input to the VM via the terminal (simulates typing). */
  private writeToShell(text: string): void {
    // slave.write() sends data TO the terminal (output direction).
    // To send input TO the VM, we go through the terminal which fires
    // onData → master → ldisc.writeFromLower → slave readable → VM reads.
    if (this.terminal) {
      // paste() triggers onData which flows through the PTY to the VM
      this.terminal.paste(text);
    }
  }

  /** Wait for the shell prompt by watching captured output. */
  private waitForPrompt(timeoutMs: number): Promise<void> {
    return new Promise((resolve, reject) => {
      const deadline = Date.now() + timeoutMs;
      const check = () => {
        if (this.outputBuffer.includes("]#") || this.outputBuffer.includes("$ ")) {
          this.outputBuffer = "";
          resolve();
          return;
        }
        if (Date.now() > deadline) {
          reject(new Error("Timed out waiting for shell prompt"));
          return;
        }
        setTimeout(check, 300);
      };
      check();
    });
  }
}
