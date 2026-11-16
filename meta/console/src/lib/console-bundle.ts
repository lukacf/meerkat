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

// One form for the version: the build gives the app bundle a version of
// lowercase hex digits and fails otherwise (`build.cjs`), so every reader of
// it (this page, the gateway's `console_bundle`, the freshness check) cuts
// the same text. A version in another form would be cut differently here
// and by the gateway, and the notice would never go away.
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

/** What the notice of another build says. The gateway's own page is cured by
 *  a reload, whichever way the gateway's build moved (an upgrade or a
 *  rollback). In an application that mounts the console's library (only the
 *  library's build names its bundle) a reload helps only when the
 *  application itself was updated meanwhile, so the notice also says what
 *  is needed when it stays. */
export function consoleBundleNotice(): string {
  return builtBeside() !== undefined
    ? "This Console is of another build than its gateway's. Reload the page; if this notice stays, the application needs the Console of its gateway's release."
    : "This page is of another Console build than its gateway's. Reload the page to use the gateway's version.";
}
