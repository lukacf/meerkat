/**
 * WebCM Host — programmatic bridge to a Linux VM running in the browser.
 *
 * Uses xterm-pty to communicate with the Cartesi RISC-V emulator.
 * VM output is captured via master.onWrite (data flowing from VM to terminal).
 * Commands use unique delimiters to detect completion and extract output.
 */

import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { openpty } from "xterm-pty";

export interface ExecResult {
  output: string;
  exitCode: number;
}

export class WebCMHost {
  private slave: any = null;
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
    const termios = slave.ioctl("TCGETS");
    termios.iflag &= ~0x5eb;  // clear BRKINT|ICRNL|INPCK|ISTRIP|IXON
    termios.cflag &= ~0x130;  // clear CSIZE|PARENB
    termios.lflag &= ~0x804b; // clear ECHO|ICANON|IEXTEN|ISIG
    termios.cflag |= 0x30;    // set CS8
    termios.oflag |= 0x1;     // set OPOST
    slave.ioctl("TCSETS", termios);

    // Capture VM output via master.onWrite. This event fires with
    // [Uint8Array, callback] for every chunk the VM writes to stdout.
    // We MUST subscribe BEFORE loadAddon, because activate() also
    // subscribes — our listener runs alongside the terminal writer.
    const decoder = new TextDecoder();
    master.onWrite(([data, _cb]: [Uint8Array, () => void]) => {
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

    // Clear buffer, send command + capture exit code + delimiter on one line
    this.outputBuffer = "";
    const wrapped = `${command} 2>&1; echo "${delim}:$?"`;
    this.writeToShell(wrapped + "\n");

    // Poll for delimiter in captured output.
    // Match "<delim>:<digit>" to avoid matching the echoed command which has "<delim>:$?".
    const escapedDelim = delim.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    const delimPattern = new RegExp(escapedDelim + ":\\d");
    const result = await new Promise<string>((resolve, reject) => {
      const deadline = Date.now() + timeoutMs;
      const check = () => {
        if (delimPattern.test(this.outputBuffer)) {
          // Wait a tick for the rest of the line
          setTimeout(() => resolve(this.outputBuffer), 50);
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

    // Parse output. Buffer format (single-line command):
    //   <echoed-wrapped-command>\r\n<command-output>\r\n<delim>:<exitcode>\r\n<prompt>
    // Strategy: strip ANSI codes, split on \n, find delimiter line,
    // skip the first line (echoed command), take everything until delimiter.
    const clean = result.replace(/\x1b\[[0-9;]*[a-zA-Z]/g, ""); // strip ANSI
    const lines = clean.split(/\r?\n/);

    // Find the delimiter OUTPUT line (contains <delim>:<digit>)
    const delimRe = new RegExp(escapedDelim + ":(\\d+)");
    let delimLineIdx = -1;
    let exitCode = 0;
    for (let i = lines.length - 1; i >= 0; i--) {
      const match = lines[i].match(delimRe);
      if (match) {
        delimLineIdx = i;
        exitCode = parseInt(match[1], 10);
        break;
      }
    }
    if (delimLineIdx < 0) return { output: "", exitCode: -1 };

    // Find the end of the echoed command. The echoed command may wrap across
    // multiple terminal lines. Look for the last line containing $?" (the
    // literal echo of our delimiter command) before the actual output.
    let cmdEndIdx = 0;
    for (let i = 0; i < delimLineIdx; i++) {
      if (lines[i].includes('$?"') || lines[i].includes(delim.slice(0, 10))) {
        cmdEndIdx = i + 1;
      }
    }

    const outputLines = lines.slice(cmdEndIdx, delimLineIdx);
    const output = outputLines.join("\n").trim();
    return { output, exitCode };
  }

  /** Write content to a file in the VM. */
  async writeFile(path: string, content: string): Promise<ExecResult> {
    // Base64 encode and split into chunks to avoid PTY buffer overflow.
    // terminal.paste() processes each character through the line discipline,
    // so very long single-line commands time out.
    const b64 = btoa(unescape(encodeURIComponent(content)));
    const CHUNK = 512;
    if (b64.length <= CHUNK) {
      return this.exec(`echo '${b64}' | base64 -d > ${path}`);
    }
    // Multi-chunk: write base64 to a temp file in chunks, then decode
    const tmp = `/tmp/_mkt_b64_${Date.now()}`;
    await this.exec(`true > ${tmp}`);
    for (let i = 0; i < b64.length; i += CHUNK) {
      const chunk = b64.slice(i, i + CHUNK);
      await this.exec(`echo -n '${chunk}' >> ${tmp}`);
    }
    const result = await this.exec(`base64 -d < ${tmp} > ${path} && rm ${tmp}`);
    return result;
  }

  /** Read a file from the VM. */
  async readFile(path: string): Promise<string> {
    const { output, exitCode } = await this.exec(`cat ${path}`);
    if (exitCode !== 0) throw new Error(`Failed to read ${path}: ${output}`);
    return output;
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
