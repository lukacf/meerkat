/**
 * File tree — browses the WebCM VM filesystem.
 */

import type { WebCMHost } from "../webcm-host";

export interface FileTreeCallbacks {
  onFileSelect: (path: string) => void;
}

interface TreeNode {
  name: string;
  path: string;
  isDir: boolean;
  children?: TreeNode[];
  depth: number;
}

export class FileTree {
  private container: HTMLElement;
  private vm: WebCMHost;
  private callbacks: FileTreeCallbacks;
  private activePath: string | null = null;
  private roots = ["/root", "/tmp"];

  constructor(container: HTMLElement, vm: WebCMHost, callbacks: FileTreeCallbacks) {
    this.container = container;
    this.vm = vm;
    this.callbacks = callbacks;
  }

  async refresh(): Promise<void> {
    const nodes: TreeNode[] = [];
    for (const root of this.roots) {
      const tree = await this.scanDir(root, 0);
      if (tree) nodes.push(tree);
    }
    this.render(nodes);
  }

  setActive(path: string): void {
    this.activePath = path;
    this.container.querySelectorAll(".tree-item").forEach((el) => {
      el.classList.toggle("active", el.getAttribute("data-path") === path);
    });
  }

  private async scanDir(path: string, depth: number): Promise<TreeNode | null> {
    if (depth > 3) return null;
    try {
      const { output } = await this.vm.exec(`ls -1a ${path} 2>/dev/null`);
      const entries = output
        .split("\n")
        .map((s) => s.trim())
        .filter((s) => s && s !== "." && s !== "..");

      const children: TreeNode[] = [];
      for (const name of entries) {
        const fullPath = path === "/" ? `/${name}` : `${path}/${name}`;
        const { exitCode } = await this.vm.exec(`test -d ${fullPath}`);
        const isDir = exitCode === 0;
        if (isDir && depth < 2) {
          const sub = await this.scanDir(fullPath, depth + 1);
          if (sub) children.push(sub);
        } else {
          children.push({ name, path: fullPath, isDir, depth: depth + 1 });
        }
      }

      return {
        name: path.split("/").pop() || path,
        path,
        isDir: true,
        children,
        depth,
      };
    } catch {
      return null;
    }
  }

  private render(nodes: TreeNode[]): void {
    this.container.innerHTML = "";
    for (const node of nodes) {
      this.renderNode(node);
    }
  }

  private renderNode(node: TreeNode): void {
    const el = document.createElement("div");
    el.className = `tree-item${node.isDir ? " dir" : ""}${node.path === this.activePath ? " active" : ""}`;
    el.setAttribute("data-path", node.path);
    el.style.paddingLeft = `${8 + node.depth * 14}px`;

    const icon = document.createElement("span");
    icon.className = "tree-icon";
    icon.textContent = node.isDir ? (node.children?.length ? "v" : ">") : " ";
    el.appendChild(icon);

    const label = document.createElement("span");
    label.textContent = node.name;
    el.appendChild(label);

    if (!node.isDir) {
      el.addEventListener("click", () => this.callbacks.onFileSelect(node.path));
    }

    this.container.appendChild(el);

    if (node.children) {
      for (const child of node.children) {
        this.renderNode(child);
      }
    }
  }
}
