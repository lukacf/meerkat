// Which console bundle a page runs, and whether its gateway serves another.
//
// The gateway names, in the experience document, the console bundle it
// serves (`console_bundle`: the version in the page's `console-app.js?v=`).
// A page knows the bundle it runs in one of two ways: the gateway's own page
// reads it from the script it loaded; an application that mounts the
// console's library has it from that library's build, which names the bundle
// it was built beside. A console run straight from its sources knows none,
// and is never called out of date.

/** Replaced by the library's build with the id of the bundle built beside it. */
declare const __META_CONSOLE_BUNDLE__: string | undefined;

const APP_SCRIPT_VERSION = /\/console-app\.js\?(?:[^#]*&)?v=([0-9A-Za-z._-]+)/;

function builtBeside(): string | undefined {
  const named = typeof __META_CONSOLE_BUNDLE__ === "string" ? __META_CONSOLE_BUNDLE__.trim() : "";
  return named || undefined;
}

function loadedByThePage(page: Document): string | undefined {
  for (const script of Array.from(page.querySelectorAll("script[src]"))) {
    const match = APP_SCRIPT_VERSION.exec(script.getAttribute("src") ?? "");
    if (match) return match[1];
  }
  return undefined;
}

/** The bundle this page runs, where it can tell. */
export function runningConsoleBundle(page: Document | undefined = typeof document === "undefined" ? undefined : document): string | undefined {
  return builtBeside() ?? (page ? loadedByThePage(page) : undefined);
}

/** The bundle the gateway says it serves; undefined for a gateway that names none. */
export function servedConsoleBundle(experience: { console_bundle?: unknown } | null | undefined): string | undefined {
  const named = typeof experience?.console_bundle === "string" ? experience.console_bundle.trim() : "";
  return named || undefined;
}

/** Whether the page runs another bundle than its gateway serves. Unknown on either side: no. */
export function consoleBundleIsStale(running: string | undefined, served: string | undefined): boolean {
  return running !== undefined && served !== undefined && running !== served;
}
