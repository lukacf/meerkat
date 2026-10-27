import React, { useEffect, useMemo, useRef, useState } from "react";
import { createRoot } from "react-dom/client";
import { ConsoleWorkbench } from "../../packages/console-components/src/workbench/console-workbench";
import { TopologyPanel } from "../../packages/console-components/src/topology/topology-panel";
import { ConversationPane } from "../../packages/console-components/src/conversation/conversation-pane";
import { ConsoleComposer } from "../../packages/console-components/src/composer/console-composer";
import {
  buildConversationViewState,
  parseConversationRichBlocks,
} from "../../packages/console-core/src/index";
import "../../packages/console-components/src/styles/index.css";
import "./fonts/fonts.css";
import "./styles.css";
import { GuidedSetup } from "./guided.jsx";
import { NativeBundleSelection } from "./native-bundle-selection.jsx";
import {
  DEFAULT_MODELS,
  DRAFT_KEY,
  clone,
  slugify,
  identities,
  resolvedEdges,
  uniqueId,
  newClass,
  resizeClass,
  canResizeClass,
  classInstanceLimit,
  removeClass,
  removeConnection,
  removeStep,
  hasCycle,
  blueprintDiff,
  buildTopologyNodes,
  flowLayout,
  fingerprint,
  request,
  normalizeError,
} from "./state.mjs";

const paths = {
  spark: <path d="m12 3 2.7 6.3L21 12l-6.3 2.7L12 21l-2.7-6.3L3 12l6.3-2.7Z" />,
  arrow: <path d="M4 12h16m-6-6 6 6-6 6" />,
  plus: <path d="M12 5v14M5 12h14" />,
  close: <path d="m6 6 12 12M6 18 18 6" />,
  check: <path d="m5 12 4 4 10-10" />,
  chevron: <path d="m9 5 7 7-7 7" />,
  topology: (
    <>
      <circle cx="12" cy="5" r="3" />
      <circle cx="5" cy="19" r="3" />
      <circle cx="19" cy="19" r="3" />
      <path d="m11 8-5 8m7-8 5 8M8 19h8" />
    </>
  ),
  team: (
    <>
      <circle cx="9" cy="8" r="3" />
      <path d="M3 21v-3a6 6 0 0 1 12 0v3m1-17a3 3 0 0 1 0 6m2 4a5 5 0 0 1 3 4v3" />
    </>
  ),
  link: (
    <path
      d="m10 13 4-4m-5 8-2 2a4 4 0 0 1-6-6l5-5a4 4 0 0 1 6 0m0 8a4 4 0 0 0 6 0l5-5a4 4 0 0 0-6-6l-2 2"
      transform="translate(1 0) scale(.9)"
    />
  ),
  flow: (
    <>
      <rect x="3" y="3" width="6" height="6" rx="1" />
      <rect x="15" y="15" width="6" height="6" rx="1" />
      <path d="M9 6h6a3 3 0 0 1 3 3v6m-4-4 4 4 4-4M3 18h7" />
    </>
  ),
  database: (
    <>
      <ellipse cx="12" cy="5" rx="8" ry="3" />
      <path d="M4 5v14c0 4 16 4 16 0V5M4 12c0 4 16 4 16 0" />
    </>
  ),
  rocket: (
    <path d="m12 15-3-3m0 0c1-6 6-9 12-9 0 6-3 11-9 12m-3-3-5 1-1-1 4-5 4-1m1 9-1 5 1 1 5-4 1-4M4 16c-2 1-2 4-2 6 2 0 5 0 6-2" />
  ),
  folder: (
    <path d="M3 7V5a2 2 0 0 1 2-2h4l2 3h8a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" />
  ),
  file: (
    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8Zm0 0v6h6M8 13h8m-8 4h5" />
  ),
  code: <path d="m8 6-6 6 6 6m8-12 6 6-6 6m-3-15-2 18" />,
  search: (
    <>
      <circle cx="10" cy="10" r="6" />
      <path d="m15 15 6 6" />
    </>
  ),
  download: <path d="M12 3v12m-5-5 5 5 5-5M4 17v4h16v-4" />,
  upload: <path d="M12 16V4m-5 5 5-5 5 5M4 17v4h16v-4" />,
  undo: <path d="m8 4-5 5 5 5M3 9h11a7 7 0 0 1 0 14" />,
  copy: (
    <>
      <rect x="8" y="8" width="12" height="13" rx="2" />
      <path d="M16 8V3H3v13h5" />
    </>
  ),
  trash: <path d="M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M9 10v7m6-7v7" />,
  settings: (
    <>
      <path d="M4 7h16M4 17h16" />
      <circle cx="9" cy="7" r="3" />
      <circle cx="16" cy="17" r="3" />
    </>
  ),
  globe: (
    <>
      <circle cx="12" cy="12" r="9" />
      <ellipse cx="12" cy="12" rx="4" ry="9" />
      <path d="M3 12h18" />
    </>
  ),
  monitor: (
    <>
      <rect x="3" y="4" width="18" height="13" rx="2" />
      <path d="M8 21h8m-4-4v4" />
    </>
  ),
  cloud: <path d="M7 18a5 5 0 1 1 1-9.9 6 6 0 0 1 11.5 2A4 4 0 0 1 18 18Z" />,
  lock: (
    <>
      <rect x="5" y="10" width="14" height="11" rx="2" />
      <path d="M8 10V6a4 4 0 0 1 8 0v4m-4 5v2" />
    </>
  ),
  external: <path d="M14 3h7v7m0-7L10 14M10 3H3v18h18v-7" />,
  menu: <path d="M4 6h16M4 12h16M4 18h16" />,
};
function Icon({ name, size = 18, className = "" }) {
  return (
    <svg
      className={`icon ${className}`}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.65"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {paths[name] || paths.spark}
    </svg>
  );
}
function Button({
  children,
  icon,
  tone = "secondary",
  className = "",
  ...props
}) {
  return (
    <button className={`button ${tone} ${className}`} {...props}>
      {icon && <Icon name={icon} />} {children}
    </button>
  );
}
function Field({ label, help, children, className = "" }) {
  return (
    <label className={`field ${className}`}>
      <span>{label}</span>
      {children}
      {help && <small>{help}</small>}
    </label>
  );
}
function Badge({ children, tone = "" }) {
  return <span className={`badge ${tone}`}>{children}</span>;
}
function Toggle({ value, onChange, title, description, disabled = false }) {
  return (
    <label className={`toggle-row ${disabled ? "disabled" : ""}`}>
      <span>
        <strong>{title}</strong>
        {description && <small>{description}</small>}
      </span>
      <input
        type="checkbox"
        checked={value}
        onChange={(e) => onChange(e.target.checked)}
        disabled={disabled}
      />
      <span className="switch" />
    </label>
  );
}
function Empty({ icon = "spark", title, description, action }) {
  return (
    <div className="empty">
      <span className="empty-icon">
        <Icon name={icon} size={28} />
      </span>
      <h3>{title}</h3>
      <p>{description}</p>
      {action}
    </div>
  );
}
function SectionHeading({ eyebrow, title, description, action }) {
  return (
    <div className="section-heading">
      <div>
        {eyebrow && <span className="eyebrow">{eyebrow}</span>}
        <h1>{title}</h1>
        {description && <p>{description}</p>}
      </div>
      {action}
    </div>
  );
}
function Modal({ title, subtitle, children, onClose, wide = false }) {
  const ref = useRef();
  useEffect(() => {
    ref.current?.focus();
    const close = (e) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", close);
    return () => document.removeEventListener("keydown", close);
  }, [onClose]);
  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <section
        className={`modal ${wide ? "wide" : ""}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
        ref={ref}
      >
        <header>
          <div>
            <h2>{title}</h2>
            {subtitle && <p>{subtitle}</p>}
          </div>
          <button
            className="icon-button"
            aria-label="Close dialog"
            onClick={onClose}
          >
            <Icon name="close" />
          </button>
        </header>
        {children}
      </section>
    </div>
  );
}
export function App() {
  const [mode, setMode] = useState("guide"),
    [manualReturn, setManualReturn] = useState(null);
  const [bp, setBp] = useState(null),
    [config, setConfig] = useState(null),
    [loadError, setLoadError] = useState("");
  const [starting, setStarting] = useState(true),
    [savedStartupDraft, setSavedStartupDraft] = useState(null),
    [replaceDraft, setReplaceDraft] = useState(false),
    [draftStorageAvailable, setDraftStorageAvailable] = useState(true);
  const [tab, setTab] = useState("topology"),
    [selected, setSelected] = useState(null),
    [inspector, setInspector] = useState(false);
  const [dock, setDock] = useState(() => window.innerWidth > 800),
    [mobileNav, setMobileNav] = useState(false),
    [modal, setModal] = useState(null);
  const [savedProject, setSavedProject] = useState(null),
    [saving, setSaving] = useState(false),
    [consoleStarting, setConsoleStarting] = useState(false);
  const proposalSeen = useRef(null);
  const [currentHash, setCurrentHash] = useState("");
  const [history, setHistory] = useState([]),
    [future, setFuture] = useState([]),
    [toast, setToast] = useState("");
  const [validation, setValidation] = useState(null),
    [validating, setValidating] = useState(false),
    [error, setError] = useState("");
  const [architectDraft, setArchitectDraft] = useState(""),
    [messages, setMessages] = useState([]),
    [architectBusy, setArchitectBusy] = useState(false),
    [proposal, setProposal] = useState(null),
    [proposalBase, setProposalBase] = useState(null);
  const latest = useRef(bp),
    fileInput = useRef(null);
  const advancedOwner = useRef(null), guidedBlueprint = useRef(null);
  const startup = useRef({ mounted: false, sequence: 0, pending: null });
  const draftStorage = useRef(null);
  latest.current = bp;
  const notify = (text) => {
    setToast(text);
    window.setTimeout(() => setToast(""), 3600);
  };
  async function load(replacement) {
    if (!startup.current.mounted || startup.current.pending) return;
    const ticket = ++startup.current.sequence;
    startup.current.pending = ticket;
    const current = () => startup.current.mounted && startup.current.pending === ticket;
    let storage = null, raw = null;
    const retainedDraft = savedStartupDraft;
    try {
      storage = globalThis.localStorage;
      raw = storage.getItem(DRAFT_KEY);
    } catch {
      storage = null;
      setDraftStorageAvailable(false);
    }
    if (storage) setSavedStartupDraft(raw === null ? null : { raw });
    setStarting(true);
    setLoadError("");
    try {
      if (!storage && retainedDraft)
        throw new Error("Your saved draft could not be read. Its retained copy is still available to download; try again when browser storage is available.");
      if (replacement && (!storage || raw !== replacement.raw))
        throw new Error("The saved draft changed or could not be read. Try opening the current draft before replacing it.");
      const result = await request("/api/workbench", { timeoutMs: 30000 });
      if (!current()) return;
      let draft = result.blueprint;
      if (raw !== null && !replacement) {
        let saved;
        try { saved = JSON.parse(raw); }
        catch { throw new Error("The saved draft is not valid JSON. Download a copy to recover it, or explicitly replace this browser's draft."); }
        if (!saved || saved.schema_version !== 2)
          throw new Error("This saved draft uses an unsupported format. Download a copy to recover it, or explicitly replace this browser's draft.");
        draft = (await request("/api/blueprint/import", { method: "POST", body: saved, timeoutMs: 30000 })).blueprint;
        if (!current()) return;
      }
      if (storage && storage.getItem(DRAFT_KEY) !== raw)
        throw new Error("The saved draft changed while it was opening. Try again to open the current saved draft.");
      // A failed read or import never reaches publication or draft persistence.
      const selectedClass = draft.agent_classes[0]?.id;
      const slug = draft.project.slug;
      draftStorage.current = storage;
      setConfig(result);
      setBp(draft);
      try {
        const savedMeta = JSON.parse(storage?.getItem("mobkit-launch-project-revision") || "null");
        if (savedMeta?.slug === slug) setSavedProject(savedMeta);
      } catch { /* Revision metadata cannot invalidate an opened draft. */ }
      setSelected(selectedClass);
      setReplaceDraft(false);
    } catch (e) {
      if (current()) { setLoadError(e.message); setReplaceDraft(false); }
    } finally {
      if (startup.current.pending === ticket) {
        startup.current.pending = null;
        if (startup.current.mounted) setStarting(false);
      }
    }
  }
  function downloadDraft(raw) {
    try {
      const url = URL.createObjectURL(new Blob([raw], { type: "application/json;charset=utf-8" }));
      const link = document.createElement("a");
      link.href = url;
      link.download = "saved-launch-draft.json";
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 10000);
    } catch { setLoadError("The copy could not be downloaded. Your saved draft is unchanged."); }
  }
  useEffect(() => {
    startup.current.mounted = true;
    load();
    return () => {
      startup.current.mounted = false;
      startup.current.pending = null;
      ++startup.current.sequence;
    };
  }, []);
  useEffect(() => {
    if (bp) {
      fingerprint(bp).then(setCurrentHash);
      try {
        if (draftStorage.current) {
          draftStorage.current.setItem(DRAFT_KEY, JSON.stringify(bp));
          setDraftStorageAvailable(true);
        }
      } catch { setDraftStorageAvailable(false); }
      setValidation(null);
    }
  }, [bp]);
  useEffect(() => {
    if (!savedProject) return;
    try {
      if (draftStorage.current) draftStorage.current.setItem(
        "mobkit-launch-project-revision", JSON.stringify(savedProject));
    } catch { setDraftStorageAvailable(false); }
  }, [savedProject]);
  useEffect(() => {
    if (!architectBusy && !config?.architect?.browser_console_url) return;
    const timer = setInterval(async () => {
      try {
        const status = await request("/api/architect/status");
        setConfig((c) => ({ ...c, architect: status }));
        if (
          status.proposal &&
          JSON.stringify(status.proposal.blueprint) !== proposalSeen.current &&
          !status.busy
        ) {
          proposalSeen.current = JSON.stringify(status.proposal.blueprint);
          setProposal({
            proposal: status.proposal.blueprint,
            message: status.proposal.summary,
            changes: status.proposal.changes,
            base_hash: status.proposal.base_hash,
            fromConsole: true,
          });
          setProposalBase(clone(latest.current));
        }
      } catch {}
    }, 5000);
    return () => clearInterval(timer);
  }, [architectBusy, config?.architect?.browser_console_url]);
  useEffect(() => {
    if (!bp) return;
    const ids = bp.agent_classes.map((c) => c.id);
    if (!ids.includes(selected)) setSelected(ids[0]);
  }, [bp, selected]);
  function update(nextOrFn) {
    const current = latest.current;
    if (!current) return;
    const next =
      typeof nextOrFn === "function" ? nextOrFn(clone(current)) : nextOrFn;
    if (JSON.stringify(current) === JSON.stringify(next)) return;
    setHistory((h) => [...h.slice(-39), current]);
    setFuture([]);
    setBp(next);
    setError("");
  }
  function acceptGuidedBlueprint(next) {
    guidedBlueprint.current = next;
    // Hidden guide recovery remains authoritative without replacing a human draft.
    if (!advancedOwner.current) update(next);
  }
  async function saveRevision() {
    setSaving(true);
    setError("");
    try {
      const result = await request(`/api/projects/${bp.project.slug}`, {
        method: "PUT",
        body: {
          blueprint: bp,
          expected_revision:
            savedProject?.slug === bp.project.slug ? savedProject.revision : 0,
        },
      });
      setSavedProject({ slug: bp.project.slug, revision: result.revision });
      notify(`Saved revision ${result.revision}.`);
    } catch (e) {
      setError(e.message);
    } finally {
      setSaving(false);
    }
  }
  async function openConsole() {
    setConsoleStarting(true);
    try {
      const status = await request("/api/architect/start", {
        method: "POST",
        body: { blueprint: bp },
      });
      setConfig((c) => ({ ...c, architect: status }));
      if (status.browser_console_url)
        setModal({ type: "console", url: status.browser_console_url });
      else
        throw new Error(status.detail || "The console is not available yet.");
    } catch (e) {
      setError(e.message);
    } finally {
      setConsoleStarting(false);
    }
  }
  function undo() {
    if (!history.length) return;
    setFuture((f) => [latest.current, ...f]);
    setBp(history.at(-1));
    setHistory((h) => h.slice(0, -1));
  }
  function redo() {
    if (!future.length) return;
    setHistory((h) => [...h, latest.current]);
    setBp(future[0]);
    setFuture((f) => f.slice(1));
  }
  function editClass(patch) {
    update((next) => {
      Object.assign(
        next.agent_classes.find((c) => c.id === selected),
        patch,
      );
      return next;
    });
  }
  function addClass() {
    const c = newClass(bp);
    update((next) => {
      next.agent_classes.push(c);
      return next;
    });
    setSelected(c.id);
    setInspector(true);
    setTab("classes");
  }
  function duplicateClass(c) {
    const copy = clone(c);
    copy.id = uniqueId(
      `${c.id}-copy`,
      bp.agent_classes.map((a) => a.id),
    );
    copy.name = `${c.name} copy`;
    update((next) => {
      next.agent_classes.push(copy);
      return next;
    });
    setSelected(copy.id);
    setInspector(true);
  }
  async function validate(isCurrent) {
    const owner = advancedOwner.current;
    const ownsSelection = () => advancedOwner.current === owner &&
      (typeof isCurrent !== "function" || isCurrent());
    if (!ownsSelection()) return null;
    setValidating(true);
    setError("");
    const snapshot = JSON.stringify(bp);
    try {
      const result = await request("/api/blueprint/validate", {
        method: "POST",
        body: bp,
      });
      if (!ownsSelection()) return null;
      if (JSON.stringify(latest.current) !== snapshot)
        throw new Error(
          "The blueprint changed during validation. Review the current package again.",
        );
      setValidation(result);
      return result;
    } catch (e) {
      if (ownsSelection()) setError(e.message);
      return null;
    } finally {
      setValidating(false);
    }
  }
  async function download() {
    const owner = advancedOwner.current, selected = bp, snapshot = JSON.stringify(selected);
    const current = () => advancedOwner.current === owner && JSON.stringify(latest.current) === snapshot;
    if (!owner || !current()) return;
    const result = await validate(current);
    if (!result || !current()) return;
    try {
      const response = await fetch("/api/blueprint/package", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: snapshot,
      });
      if (!current()) return;
      if (!response.ok) throw new Error(normalizeError(await response.json()));
      const blob = await response.blob();
      if (!current()) return;
      downloadBlob(blob, `${selected.project.slug}.zip`);
      notify("Your deployment package is ready.");
    } catch (e) {
      if (current()) setError(e.message);
    }
  }
  async function importFile(event) {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;
    try {
      if (file.size > 500000)
        throw new Error("Choose a blueprint smaller than 500 KB.");
      const result = await request("/api/blueprint/import", {
        method: "POST",
        body: JSON.parse(await file.text()),
      });
      update(result.blueprint);
      notify("Blueprint imported.");
    } catch (e) {
      setError(e.message);
    }
  }
  async function sendArchitect(message) {
    if (!message.trim() || architectBusy) return;
    const base = clone(bp);
    setArchitectBusy(true);
    setArchitectDraft("");
    setError("");
    setMessages((m) => [...m, messageEntry("user", message)]);
    try {
      const response = await request("/api/architect/message", {
        method: "POST",
        body: { message, blueprint: base },
      });
      if (response.browser_console_url)
        setConfig((c) => ({
          ...c,
          architect: { ...c.architect, browser_console_url: response.browser_console_url },
        }));
      setMessages((m) => [
        ...m,
        messageEntry(
          "assistant",
          response.message || "The architecture proposal is ready to review.",
        ),
      ]);
      if (response.proposal) {
        proposalSeen.current = JSON.stringify(response.proposal);
        setProposal(response);
        setProposalBase(base);
      }
      if (response.warnings?.length)
        setMessages((m) => [
          ...m,
          messageEntry("system", response.warnings.join("\n")),
        ]);
    } catch (e) {
      setMessages((m) => [...m, messageEntry("system", e.message)]);
    } finally {
      setArchitectBusy(false);
    }
  }
  if (!bp)
    return (
      <div className="loading-page">
        <div className="brandmark">
          <Icon name="spark" size={32} />
        </div>
        <h1>Meta Forge</h1>
        <p role={loadError ? "alert" : "status"}>{loadError || "Getting your setup guide ready..."}</p>
        {savedStartupDraft && <p>Your saved draft is kept unchanged until it opens successfully or you choose to replace it. Saved conversations and installations are kept separately.</p>}
        {loadError && <Button disabled={starting} onClick={() => load()}>Try again</Button>}
        {savedStartupDraft && <>
          <Button onClick={() => downloadDraft(savedStartupDraft.raw)}>Download saved draft</Button>
          {!replaceDraft ? <Button disabled={starting} onClick={() => setReplaceDraft(true)}>Use a fresh draft instead</Button>
            : <div className="notice amber" role="group" aria-label="Replace saved browser draft">
              <p>This replaces the draft saved in this browser. Download a copy first if you need it. Your saved conversations and installed applications are kept.</p>
              <Button disabled={starting} onClick={() => load(savedStartupDraft)}>Replace saved draft</Button>
              <Button disabled={starting} onClick={() => setReplaceDraft(false)}>Keep saved draft</Button>
            </div>}
        </>}
      </div>
    );
  const members = identities(bp),
    edges = resolvedEdges(bp),
    currentClass = bp.agent_classes.find((c) => c.id === selected);
  const tabs = [
    { id: "topology", name: "Architecture", icon: "topology" },
    { id: "classes", name: "Agent classes", icon: "team" },
    { id: "tools", name: "Connections", icon: "link" },
    { id: "flows", name: "Workflows", icon: "flow" },
    { id: "knowledge", name: "Knowledge", icon: "database" },
    { id: "deploy", name: "Deployment", icon: "rocket" },
  ];
  const goTab = (id) => {
    setTab(id);
    setInspector(false);
    setMobileNav(false);
  };
  const sidebar = (
    <>
      <div className="nav-group">
        <span className="sidebar-caption">WORKSPACE</span>
        {tabs.map((t) => (
          <button
            key={t.id}
            className={`nav-item ${tab === t.id ? "active" : ""}`}
            onClick={() => goTab(t.id)}
          >
            <Icon name={t.icon} />
            <span>{t.name}</span>
            {t.id === "tools" && bp.connections.length > 0 && (
              <b>{bp.connections.length}</b>
            )}
            {t.id === "flows" && bp.flows.length > 0 && (
              <b>{bp.flows.length}</b>
            )}
          </button>
        ))}
      </div>
      <div className="sidebar-section-title">
        <span className="sidebar-caption">AGENT CLASSES</span>
        <button
          className="icon-button"
          title="Add an agent class"
          onClick={addClass}
        >
          <Icon name="plus" size={15} />
        </button>
      </div>
      <div className="class-list">
        {bp.agent_classes.map((c, i) => (
          <button
            key={c.id}
            className={`class-row ${inspector && selected === c.id ? "selected" : ""}`}
            onClick={() => {
              setSelected(c.id);
              setInspector(true);
              setTab("classes");
              setMobileNav(false);
            }}
          >
            <Avatar index={i} label={c.name} />
            <span>
              <strong>{c.name}</strong>
              <small>{c.model}</small>
            </span>
            <span className="count-pill">{c.instances}</span>
          </button>
        ))}
      </div>
      <button className="add-row" onClick={addClass}>
        <Icon name="plus" size={16} /> New agent class
      </button>
      <div className="sidebar-note">
        <span className="small-orbit">
          <Icon name="topology" size={23} />
        </span>
        <strong>A team that works your way.</strong>
        <p>
          Shape the people, connections, and routines. Your blueprint stays
          yours.
        </p>
      </div>
    </>
  );
  const architect = (
    <div className="architect-dock">
      <div className="architect-intro">
        <span className="eyebrow">YOUR SYSTEM, CO-DESIGNED</span>
        <h2>
          Think bigger.
          <br />
          Build it together.
        </h2>
        <p>
          Your Forge architect can shape classes, find MCP servers, and connect
          the whole system.
        </p>
        <button
          className="native-console-link"
          onClick={openConsole}
          disabled={consoleStarting}
        >
          <Icon name="external" size={12} />
          {consoleStarting
            ? "Starting setup team..."
            : config?.architect?.browser_console_url
              ? "Open the live setup console"
              : "Start the Forge setup team"}
        </button>
        {!messages.length && (
          <div className="suggestion-chips">
            {[
              "Design a research team with parallel investigators and a critical reviewer.",
              "Find the right MCP connectors for GitHub and web research.",
              "Add a review workflow with an explicit quality check.",
            ].map((text, i) => (
              <button key={text} onClick={() => setArchitectDraft(text)}>
                {["Design my team", "Find connectors", "Build a workflow"][i]}
                <Icon name="arrow" size={13} />
              </button>
            ))}
          </div>
        )}
      </div>
      {proposal && (
        <div className="proposal-card">
          <div>
            <Icon name="spark" />
            <strong>Architecture proposal</strong>
            <Badge>
              {blueprintDiff(proposalBase, proposal.proposal).length} changes
            </Badge>
          </div>
          <p>Review the suggested blueprint before applying it.</p>
          <div className="button-row">
            <Button
              tone="primary"
              onClick={() => setModal({ type: "proposal" })}
            >
              Review proposal
            </Button>
            <button className="text-button" onClick={() => setProposal(null)}>
              Dismiss
            </button>
          </div>
        </div>
      )}
      <ArchitectConversation
        agent={null}
        agentLabel="Architect"
        identity="architect:main"
        entries={messages}
        draft={architectDraft}
        sending={architectBusy}
        phase={architectBusy ? "Designing your system" : null}
        targetLabel="Architect"
        modelLabel={config?.architect?.model}
        mainRowItems={[]}
        footerLeftItems={[
          { id: "runtime", kind: "sub-pill", label: "Powered by MobKit" },
        ]}
        footerRightItems={[]}
        onDraftChange={setArchitectDraft}
        onSend={sendArchitect}
        inputId="architect-message"
        submitButtonId="architect-send"
        className="architect-conversation"
      />
      {config?.architect?.available === false && (
        <div className="architect-status">
          {config.architect.detail ||
            "Set a provider API key on the local server to enable the architect."}
        </div>
      )}
    </div>
  );
  const main = (
    <div className={`main-content ${inspector ? "inspecting" : ""}`}>
      {error && (
        <div className="error-banner" role="alert">
          <span>{error}</span>
          <button aria-label="Dismiss error" onClick={() => setError("")}>
            <Icon name="close" />
          </button>
        </div>
      )}
      {tab === "topology" && (
        <Architecture
          bp={bp}
          update={update}
          setSelected={(id) => {
            setSelected(id);
            setInspector(true);
            setTab("classes");
          }}
          setModal={setModal}
          goTab={goTab}
          onAsk={() => {
            setDock(true);
            document.getElementById("architect-message")?.focus();
          }}
        />
      )}
      {tab === "classes" && (
        <div className="section-scroll">
          <SectionHeading
            eyebrow="DEFINE THE TEAM"
            title="Give every agent a purpose."
            description="Classes share instructions, model, and tools. Scale each into as many durable agents as you need."
            action={
              <Button icon="plus" onClick={addClass}>
                New class
              </Button>
            }
          />
          <div className={inspector ? "classes-with-inspector" : ""}>
            <div className="class-cards">
              {bp.agent_classes.map((c, i) => (
                <ClassCard
                  key={c.id}
                  c={c}
                  index={i}
                  selected={inspector && selected === c.id}
                  onClick={() => {
                    setSelected(c.id);
                    setInspector(true);
                  }}
                  onDuplicate={() => duplicateClass(c)}
                />
              ))}
            </div>
            {inspector && currentClass && (
              <ClassInspector
                key={selected}
                c={currentClass}
                bp={bp}
                onClose={() => setInspector(false)}
                onChange={editClass}
                onResize={(n) => update(resizeClass(bp, selected, n))}
                onDelete={() => {
                  update(removeClass(bp, selected));
                  setInspector(false);
                }}
                onDuplicate={() => duplicateClass(currentClass)}
              />
            )}
          </div>
        </div>
      )}
      {tab === "tools" && (
        <Connections bp={bp} update={update} setModal={setModal} />
      )}
      {tab === "flows" && <Flows bp={bp} update={update} />}
      {tab === "knowledge" && <Knowledge bp={bp} update={update} />}
      {tab === "deploy" && (
        <Deployment
          bp={bp}
          update={update}
          config={config}
          validation={validation}
          validating={validating}
          validate={validate}
          download={download}
        />
      )}
    </div>
  );
  return (
    <>
      {!draftStorageAvailable && <div className="notice amber" role="status">
        This browser cannot save your local draft. Keep this page open and download a copy before closing.
        <Button onClick={() => downloadDraft(JSON.stringify(latest.current, null, 2))}>Download draft</Button>
      </div>}
      <GuidedSetup
        active={mode === "guide"}
        bp={bp}
        onBlueprint={acceptGuidedBlueprint}
        onAdvanced={(draft, editorId) => {
          advancedOwner.current = { id: editorId };
          update(draft);
          setMode("advanced");
          setDock(false);
          setTab("topology");
        }}
        manualReturn={manualReturn}
        config={config}
        Icon={Icon}
        Button={Button}
        Conversation={ArchitectConversation}
        makeEntry={messageEntry}
        onSave={saveRevision}
        saving={saving}
        savedProject={savedProject}
        appError={error}
        appToast={toast}
        onDismissError={() => setError("")}
      />
      <div
        data-cc-theme="light"
        style={mode === "guide" ? { display: "none" } : undefined}
        className={`app-shell ${!dock ? "dock-closed" : ""} ${mobileNav ? "mobile-nav-open" : ""}`}
      >
        <header className="app-header">
          <button
            className="icon-button mobile-menu"
            aria-label="Toggle navigation"
            onClick={() => setMobileNav(!mobileNav)}
          >
            <Icon name="menu" />
          </button>
          <div className="brand">
            <span className="brandmark">
              <Icon name="spark" size={22} />
            </span>
            <strong>
              Meta <span>Forge</span>
            </strong>
            <Badge>ADVANCED CONTROLS</Badge>
          </div>
          <span className="header-divider" />
          <button
            className="project-switch"
            onClick={() => setModal({ type: "project" })}
          >
            {bp.project.name}
            <Icon name="settings" size={14} />
          </button>
          <div className="header-right">
            <span className="save-state">
              <span /> Draft on this device
            </span>
            <Button
              tone="ghost"
              icon="check"
              className="save-revision"
              onClick={saveRevision}
              disabled={saving}
            >
              {saving ? "Saving..." : "Save revision"}
            </Button>
            <button
              className="icon-button"
              aria-label="Undo last change"
              onClick={undo}
              disabled={!history.length}
            >
              <Icon name="undo" size={16} />
            </button>
            <button
              className="icon-button redo"
              aria-label="Redo last change"
              onClick={redo}
              disabled={!future.length}
            >
              <Icon name="undo" size={16} />
            </button>
            <button
              className={`icon-button ${dock ? "toggled" : ""}`}
              aria-label="Toggle architect"
              onClick={() => setDock(!dock)}
            >
              <Icon name="spark" size={17} />
            </button>
            <Button
              icon="rocket"
              tone="primary"
              onClick={() => goTab("deploy")}
            >
              Deploy<span className="hide-small"> your system</span>
            </Button>
          </div>
        </header>
        <div className="guide-return-bar">
          <Button
            icon="arrow"
            tone="primary"
            onClick={() => {
              setManualReturn({
                id: crypto.randomUUID(),
                editor_id: advancedOwner.current?.id,
                blueprint: clone(bp),
              });
              advancedOwner.current = null;
              if (guidedBlueprint.current) update(guidedBlueprint.current);
              setMode("guide");
            }}
          >
            Back to guided setup
          </Button>
          <span>
            Make any adjustments here. Your guide will review your changes when
            you return.
          </span>
        </div>
        <ConsoleWorkbench
          className="launch-workbench"
          launcher={sidebar}
          launcherHeader={
            <button
              className="project-card"
              onClick={() => setModal({ type: "project" })}
            >
              <span className="project-avatar">
                {bp.project.name.charAt(0).toUpperCase()}
              </span>
              <span>
                <strong>{bp.project.name}</strong>
                <small>Personal workspace</small>
              </span>
              <Icon name="chevron" size={13} />
            </button>
          }
          launcherFooter={
            <>
              <Button
                tone="ghost"
                icon="check"
                className="sidebar-save"
                onClick={saveRevision}
                disabled={saving}
              >
                {saving ? "Saving..." : "Save revision"}
              </Button>
              <button
                className="projects-link"
                onClick={() => setModal({ type: "projects" })}
              >
                <Icon name="folder" size={14} /> Projects & history{" "}
                {savedProject && <Badge>r{savedProject.revision}</Badge>}
              </button>
              <div className="sidebar-bottom">
                <Button
                  icon="upload"
                  tone="ghost"
                  onClick={() => fileInput.current?.click()}
                >
                  Import
                </Button>
                <Button
                  icon="download"
                  tone="ghost"
                  onClick={() => {
                    downloadBlob(
                      new Blob([JSON.stringify(bp, null, 2)], {
                        type: "application/json",
                      }),
                      `${bp.project.slug}.blueprint.json`,
                    );
                  }}
                >
                  Export
                </Button>
              </div>
              <div className="engine-label">
                <span /> Meerkat + MobKit
              </div>
            </>
          }
          mainHeader={
            <div className="workspace-toolbar">
              <div>
                <span className="breadcrumb">{bp.project.name}</span>
                <span>/</span>
                <strong>{tabs.find((t) => t.id === tab)?.name}</strong>
              </div>
              <div>
                <Badge tone="green">Draft blueprint</Badge>
                <button
                  className="text-button"
                  onClick={() => setModal({ type: "json" })}
                >
                  <Icon name="code" size={15} /> Source
                </button>
              </div>
            </div>
          }
          main={main}
          mainFooter={
            <div className="statusbar">
              <span>
                <span className="status-dot" />
                {members.length} agents <i /> {bp.agent_classes.length} classes{" "}
                <i /> {edges.length} links
              </span>
              <span>
                {bp.connections.length} MCP connections <i />{" "}
                {bp.target === "mac"
                  ? "macOS"
                  : bp.target === "gcp"
                    ? "Google Cloud"
                    : "Linux"}
              </span>
            </div>
          }
          activityRail={dock ? architect : null}
          activityRailHeader={
            dock && (
              <div className="dock-header">
                <span>
                  <Icon name="spark" />
                  <strong>System architect</strong>
                </span>
                <div>
                  <button
                    className="icon-button"
                    title="Open live MobKit console"
                    aria-label="Open live MobKit console"
                    disabled={consoleStarting}
                    onClick={openConsole}
                  >
                    <Icon name="external" size={15} />
                  </button>
                  <button
                    className="icon-button"
                    aria-label="Close architect"
                    onClick={() => setDock(false)}
                  >
                    <Icon name="close" size={15} />
                  </button>
                </div>
              </div>
            )
          }
        />
        <input
          ref={fileInput}
          type="file"
          accept="application/json,.json"
          hidden
          onChange={importFile}
        />
        {toast && (
          <div className="toast" role="status">
            <Icon name="check" />
            {toast}
          </div>
        )}
        {modal?.type === "projects" && (
          <ProjectsModal
            onClose={() => setModal(null)}
            onLoad={(next, revision) => {
              update(next);
              setSavedProject({ slug: next.project.slug, revision });
              setModal(null);
              notify("Project loaded.");
            }}
          />
        )}
        {modal?.type === "console" && (
          <Modal
            wide
            title="Live MobKit setup console"
            subtitle="The real architect, connector specialist, and reviewer. Proposals return to this workbench for review."
            onClose={() => setModal(null)}
          >
            <div className="console-launch">
              <Icon name="topology" size={40} />
              <h3>Your setup team is running.</h3>
              <p>
                Open the native console to inspect agent conversations, tools,
                and topology. Its proposed blueprint will appear in the
                architect dock.
              </p>
              <a
                className="button primary"
                href={modal.url}
                target="_blank"
                rel="noreferrer"
              >
                Open MobKit console
                <Icon name="external" size={15} />
              </a>
              <code>{modal.url}</code>
            </div>
            <footer>
              <Button onClick={() => setModal(null)}>Back to workbench</Button>
            </footer>
          </Modal>
        )}
        {modal?.type === "project" && (
          <ProjectModal
            bp={bp}
            onClose={() => setModal(null)}
            onApply={(next) => {
              update(next);
              setModal(null);
            }}
          />
        )}
        {modal?.type === "json" && (
          <JsonModal
            bp={bp}
            onClose={() => setModal(null)}
            onApply={(next) => {
              update(next);
              setModal(null);
              notify("Blueprint updated.");
            }}
          />
        )}
        {modal?.type === "edges" && (
          <EdgeModal
            bp={bp}
            onClose={() => setModal(null)}
            onApply={(edges) => {
              update((next) => {
                next.topology.mode = "manual";
                next.topology.edges = edges;
                return next;
              });
              setModal(null);
            }}
          />
        )}
        {modal?.type === "inspect" && (
          <InspectToolsModal
            connection={modal.connection}
            bp={bp}
            update={update}
            onClose={() => setModal(null)}
          />
        )}
        {modal?.type === "connector" && (
          <ConnectorModal
            bp={bp}
            connection={modal.connection}
            onClose={() => setModal(null)}
            onApply={(connection) => {
              update((next) => {
                const index = next.connections.findIndex(
                  (c) => c.id === connection.id,
                );
                if (index >= 0) next.connections[index] = connection;
                else next.connections.push(connection);
                return next;
              });
              setModal(null);
              notify(
                "Connection added. Assign it to an agent class to grant access.",
              );
            }}
          />
        )}
        {modal?.type === "proposal" && proposal && (
          <Modal
            wide
            title="Review your architecture"
            subtitle="Every change is part of the blueprint. Nothing is deployed by applying this proposal."
            onClose={() => setModal(null)}
          >
            <div className="modal-body">
              <div className="proposal-summary">
                <Icon name="spark" size={24} />
                <p>{proposal.message}</p>
              </div>
              <div className="diff-list">
                {blueprintDiff(proposalBase, proposal.proposal).map((text) => (
                  <div key={text}>
                    <Icon name="check" size={16} />
                    {text}
                  </div>
                ))}
              </div>
              {(proposal.base_hash
                ? proposal.base_hash !== currentHash
                : JSON.stringify(bp) !== JSON.stringify(proposalBase)) && (
                <div className="notice amber">
                  This proposal may be based on an earlier draft. Applying it
                  replaces the current blueprint. Export your draft first to
                  keep a separate copy, or ask the architect again with the
                  current blueprint.
                </div>
              )}
              <details>
                <summary>Inspect proposed blueprint</summary>
                <pre className="code-block">
                  {JSON.stringify(proposal.proposal, null, 2)}
                </pre>
              </details>
            </div>
            <footer>
              <Button onClick={() => setModal(null)}>Keep editing</Button>
              <Button
                tone="primary"
                icon="check"
                onClick={() => {
                  update(proposal.proposal);
                  setProposal(null);
                  setModal(null);
                  notify("Architecture proposal applied.");
                }}
              >
                Apply proposal
              </Button>
            </footer>
          </Modal>
        )}
      </div>
    </>
  );
}
function AllowlistInput({ values, onChange }) {
  const [raw, setRaw] = useState(values.join(", "));
  return (
    <input
      placeholder="search, get_document"
      value={raw}
      onChange={(e) => setRaw(e.target.value)}
      onBlur={() => {
        const parsed = [
          ...new Set(
            raw
              .split(",")
              .map((v) => v.trim())
              .filter(Boolean),
          ),
        ];
        onChange(parsed);
        setRaw(parsed.join(", "));
      }}
    />
  );
}
function ArchitectConversation({
  entries,
  draft,
  sending,
  phase,
  onDraftChange,
  onSend,
  modelLabel,
  guide = false,
}) {
  const viewState = buildConversationViewState({
    memberId: "architect:main",
    agentLabel: guide ? "Setup guide" : "Architect",
    agent: null,
    entries,
  });
  const composer = {
    value: draft,
    placeholder: guide
      ? "Ask a question or tell me more..."
      : "Describe the system you want to build...",
    disabled: sending,
    submitDisabled: sending || !draft.trim(),
    submitLabel: guide ? "Send to your guide" : "Send to Architect",
    mainRowItems: [],
    footerLeftItems: [
      {
        id: "runtime",
        kind: "sub-pill",
        label: modelLabel
          ? `Forge ${guide ? "guide" : "architect"} · ${modelLabel}`
          : "Powered by MobKit",
      },
    ],
    footerRightItems: [],
  };
  return (
    <div className="architect-conversation cc-conversation-panel">
      <ConversationPane viewState={viewState} showTurnRail={false} />
      {phase && (
        <div className="cc-conversation-panel__phase">
          <span className="status-dot" />
          {phase}
        </div>
      )}
      <ConsoleComposer
        viewState={composer}
        inputId={guide ? "guide-message" : "architect-message"}
        submitButtonId={guide ? "guide-send" : "architect-send"}
        onChange={onDraftChange}
        onSubmit={() => onSend(draft)}
      />
    </div>
  );
}
function Avatar({ index = 0, label, size = "" }) {
  return (
    <span className={`avatar color-${index % 6} ${size}`}>
      {label
        .split(/\s+/)
        .filter(Boolean)
        .slice(0, 2)
        .map((x) => x[0])
        .join("")
        .toUpperCase()}
    </span>
  );
}
function messageEntry(role, text) {
  return {
    id: crypto.randomUUID(),
    kind: "message",
    variant: role === "assistant" ? "rich" : "plain",
    ...(role === "assistant"
      ? { blocks: parseConversationRichBlocks(text) }
      : {}),
    text,
    createdAt: new Date().toISOString(),
    identity: {
      id: role,
      label:
        role === "user" ? "You" : role === "assistant" ? "Architect" : "Status",
      role,
      presentation: role === "system" ? "system" : role,
    },
  };
}
function downloadBlob(blob, filename) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 10000);
}
function Architecture({ bp, update, setSelected, setModal, goTab, onAsk }) {
  const nodes = useMemo(() => buildTopologyNodes(bp), [bp]);
  const members = identities(bp),
    edgeCount = resolvedEdges(bp).length;
  return (
    <div className="architecture-page">
      <div className="architecture-intro">
        <div>
          <span className="eyebrow">FROM A GOOD IDEA TO A GREAT SYSTEM</span>
          <h1>Your agents. Your architecture.</h1>
          <p>{bp.project.description}</p>
        </div>
        <button
          className="architect-nudge"
          aria-label="Build with an architect"
          onClick={onAsk}
        >
          <span>
            <Icon name="spark" size={24} />
          </span>
          <div>
            <strong>Build with an architect</strong>
            <small>
              Describe what you're imagining
              <Icon name="arrow" size={13} />
            </small>
          </div>
        </button>
      </div>
      <div className="overview-metrics">
        <div>
          <Icon name="team" />
          <strong>{bp.agent_classes.length}</strong>
          <span>agent classes</span>
        </div>
        <div>
          <Icon name="topology" />
          <strong>{members.length}</strong>
          <span>durable agents</span>
        </div>
        <div>
          <Icon name="link" />
          <strong>{bp.connections.length}</strong>
          <span>MCP connections</span>
        </div>
        <div>
          <Icon name="flow" />
          <strong>{bp.flows.length}</strong>
          <span>workflows</span>
        </div>
      </div>
      <div className="architecture-canvas">
        <div className="canvas-toolbar">
          <span className="canvas-label">
            <span /> COMMUNICATION TOPOLOGY
          </span>
          <div className="segmented" aria-label="Topology pattern">
            {[
              ["hub", "Hub & spoke"],
              ["mesh", "Mesh"],
              ["pipeline", "Pipeline"],
              ["manual", "Custom"],
            ].map(([mode, label]) => (
              <button
                key={mode}
                className={bp.topology.mode === mode ? "active" : ""}
                onClick={() =>
                  update((next) => {
                    if (mode === "manual" && next.topology.mode !== "manual")
                      next.topology.edges = resolvedEdges(next);
                    next.topology.mode = mode;
                    return next;
                  })
                }
              >
                {label}
              </button>
            ))}
          </div>
          <button
            className="text-button"
            onClick={() => setModal({ type: "edges" })}
          >
            <Icon name="settings" size={14} /> Edit links
          </button>
        </div>
        <TopologyPanel
          nodes={nodes}
          agents={[]}
          activity={[]}
          title="Team map"
          defaultView="graph"
        />
        <div className="canvas-bottom">
          <span>
            <Icon name="topology" size={14} /> {edgeCount} peer links{" "}
            <span className="subtle">- draft preview</span>
          </span>
          <label>
            Entry point{" "}
            <select
              aria-label="Entry point"
              value={bp.topology.entrypoint}
              onChange={(e) =>
                update((next) => {
                  next.topology.entrypoint = e.target.value;
                  return next;
                })
              }
            >
              {members.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </label>
        </div>
      </div>
      <div className="under-canvas">
        <div className="under-canvas-heading">
          <span className="eyebrow">THE BUILDING BLOCKS</span>
          <button className="text-button" onClick={() => goTab("classes")}>
            All agent classes
            <Icon name="arrow" size={14} />
          </button>
        </div>
        <div className="compact-class-grid">
          {bp.agent_classes.map((c, i) => (
            <button
              className="compact-class"
              key={c.id}
              onClick={() => setSelected(c.id)}
            >
              <div>
                <Avatar index={i} label={c.name} />
                <Badge>
                  {c.instances} {c.instances === 1 ? "agent" : "agents"}
                </Badge>
              </div>
              <strong>{c.name}</strong>
              <p>{c.description}</p>
              <span>
                {toolCount(c)} {c.tools.rust_bundles?.length ? (toolCount(c) === 1 ? "tool selection" : "tool selections") : "tool grants"}
                <Icon name="arrow" size={14} />
              </span>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
function toolCount(c) {
  return (
    Number(c.tools.workspace) +
    Number(c.tools.notes !== "none") +
    Number(c.tools.builtins) +
    Number(c.tools.shell || false) +
    Number(c.tools.memory ?? true) +
    Number(c.tools.workgraph || false) +
    Number(c.tools.schedule || false) +
    Number(c.tools.knowledge) +
    c.tools.mcp.length +
    (c.tools.rust_bundles?.length || 0)
  );
}
function ClassCard({ c, index, selected, onClick, onDuplicate }) {
  return (
    <article className={`class-card ${selected ? "selected" : ""}`}>
      <button className="class-card-main" onClick={onClick}>
        <div>
          <Avatar index={index} label={c.name} size="large" />
          <Badge>
            {c.instances} {c.instances === 1 ? "instance" : "instances"}
          </Badge>
        </div>
        <h3>{c.name}</h3>
        <p>{c.description}</p>
        <div className="class-tags">
          <Badge>{c.provider}</Badge>
          <Badge>{toolCount(c)} {c.tools.rust_bundles?.length ? (toolCount(c) === 1 ? "selection" : "selections") : "grants"}</Badge>
          {c.tools.read_only && <Badge>Read-only policy</Badge>}
          {c.tools.knowledge && <Badge tone="purple">Knowledge</Badge>}
        </div>
        <div className="class-model">
          <Icon name="spark" size={14} />
          {c.model}
        </div>
      </button>
      <footer>
        <code>{c.id}</code>
        <button
          className="icon-button"
          aria-label={`Duplicate ${c.name}`}
          onClick={onDuplicate}
        >
          <Icon name="copy" size={15} />
        </button>
      </footer>
    </article>
  );
}
function ClassInspector({
  c,
  bp,
  onChange,
  onResize,
  onClose,
  onDelete,
  onDuplicate,
}) {
  const [subtab, setSubtab] = useState("profile");
  const toolChange = (patch) => onChange({ tools: { ...c.tools, ...patch } });
  return (
    <aside className="class-inspector">
      <header>
        <div>
          <span className="eyebrow">CLASS DEFINITION</span>
          <h2>{c.name}</h2>
        </div>
        <button
          className="icon-button"
          aria-label="Close class inspector"
          onClick={onClose}
        >
          <Icon name="close" />
        </button>
      </header>
      <div className="inspector-tabs">
        {["profile", "instructions", "tools"].map((id) => (
          <button
            key={id}
            className={subtab === id ? "active" : ""}
            onClick={() => setSubtab(id)}
          >
            {id}
          </button>
        ))}
      </div>
      <div className="inspector-body">
        {subtab === "profile" && (
          <>
            <Field label="Class name">
              <input
                value={c.name}
                onChange={(e) => onChange({ name: e.target.value })}
                maxLength={60}
              />
            </Field>
            <Field label="Responsibility">
              <textarea
                value={c.description}
                onChange={(e) => onChange({ description: e.target.value })}
                rows={3}
              />
            </Field>
            <Field
              label="Number of instances"
              help="Each initial agent has its own durable conversation. Zero retains this reusable class for real contexts created later; entrypoints and fixed workflow targets must remain."
            >
              <div className="stepper">
                <button
                  aria-label="Fewer instances"
                  disabled={!canResizeClass(bp, c.id, c.instances - 1)}
                  onClick={() => onResize(c.instances - 1)}
                >
                  -
                </button>
                <input
                  aria-label="Instance count"
                  type="number"
                  min="0"
                  max={classInstanceLimit(bp, c.id)}
                  value={c.instances}
                  onChange={(e) => onResize(e.target.valueAsNumber)}
                />
                <button
                  aria-label="More instances"
                  disabled={c.instances >= classInstanceLimit(bp, c.id)}
                  onClick={() => onResize(c.instances + 1)}
                >
                  +
                </button>
                <small>
                  {c.instances === 0
                    ? "Reusable class - no initial identities"
                    : c.instances === 1
                    ? `${c.id}:main`
                    : `${c.id}:1 ... ${c.id}:${c.instances}`}
                </small>
              </div>
            </Field>
            <div className="field-divider" />
            <Field label="Model provider">
              <select
                value={c.provider}
                onChange={(e) =>
                  onChange({
                    provider: e.target.value,
                    model: DEFAULT_MODELS[e.target.value],
                  })
                }
              >
                <option value="openai">OpenAI</option>
                <option value="anthropic">Anthropic</option>
                <option value="gemini">Google Gemini</option>
              </select>
            </Field>
            <Field
              label="Model"
              help="Use a model available to your provider account."
            >
              <input
                value={c.model}
                onChange={(e) => onChange({ model: e.target.value })}
              />
            </Field>
            <div className="notice">
              <Icon name="lock" size={15} />
              <span>
                Provider credentials are collected locally during installation.
              </span>
            </div>
          </>
        )}
        {subtab === "instructions" && (
          <>
            <Field
              label="Agent instructions"
              help="Shared by all instances in this class."
            >
              <textarea
                className="prompt-editor"
                value={c.instructions}
                rows={15}
                onChange={(e) => onChange({ instructions: e.target.value })}
              />
            </Field>
            <div className="notice">
              Describe the outcome, working style, and boundaries. The runtime
              adds the actual tools and wired peers.
            </div>
          </>
        )}
        {subtab === "tools" && (
          <>
            <span className="eyebrow">NATIVE EXECUTION POLICY</span>
            <Toggle
              value={c.tools.read_only ?? false}
              onChange={(v) => toolChange({ read_only: v })}
              title="Native read-only policy"
              description="Declares a native policy for new identities that blocks mutating tools and callbacks without a read-only declaration. Keeps grants and peer wiring. Reads can send network queries; this is not a privacy guarantee. Existing identity policy changes need destination verification; clearing this flag does not prove unrestricted access."
            />
            <div className="field-divider" />
            <span className="eyebrow">LOCAL CAPABILITIES</span>
            <Toggle
              value={c.tools.workspace}
              onChange={(v) => toolChange({ workspace: v })}
              title="Workspace files"
              description="Read files in the deployment workspace."
            />
            <Field label="Shared SQLite notes">
              <select
                value={c.tools.notes}
                onChange={(e) => toolChange({ notes: e.target.value })}
              >
                <option value="none">No access</option>
                <option value="read">Read notes</option>
                <option value="write">Read and write notes</option>
              </select>
            </Field>
            <Toggle
              value={c.tools.builtins}
              onChange={(v) => toolChange({ builtins: v })}
              title="Built-in tools"
              description="Runtime-provided filesystem and utility tools."
            />
            <Toggle
              value={c.tools.shell || false}
              onChange={(v) => toolChange({ shell: v })}
              title="Shell execution"
              description="Allow commands on the deployment host."
            />
            <Toggle
              value={c.tools.memory ?? true}
              onChange={(v) => toolChange({ memory: v })}
              title="Agent memory"
              description="Persistent memory across conversations."
            />
            <Toggle
              value={c.tools.workgraph || false}
              onChange={(v) => toolChange({ workgraph: v })}
              title="WorkGraph tools"
              description="Plan and track work through native WorkGraph."
            />
            <Toggle
              value={c.tools.schedule || false}
              onChange={(v) => toolChange({ schedule: v })}
              title="Scheduling tools"
              description="Create durable scheduled work."
            />
            <Toggle
              value={c.tools.knowledge}
              onChange={(v) => toolChange({ knowledge: v })}
              title="Knowledge service"
              description={
                bp.knowledge.mode === "none"
                  ? "Configure a knowledge base to enable this."
                  : "Access the configured knowledge space."
              }
              disabled={bp.knowledge.mode === "none" && !c.tools.knowledge}
            />
            <div className="field-divider" />
            <NativeBundleSelection tools={c.tools} />
            <span className="eyebrow">MCP TOOL GRANTS</span>
            {!bp.connections.length && (
              <p className="help-text">
                Add MCP connections in Connections, then choose exactly which
                classes can use them.
              </p>
            )}
            {bp.connections.map((connection) => {
              const grant = c.tools.mcp.find(
                (g) => g.connection_id === connection.id,
              );
              return (
                <div className="grant-card" key={connection.id}>
                  <Toggle
                    value={!!grant}
                    onChange={(v) =>
                      toolChange({
                        mcp: v
                          ? [
                              ...c.tools.mcp,
                              { connection_id: connection.id, tool_names: [] },
                            ]
                          : c.tools.mcp.filter(
                              (g) => g.connection_id !== connection.id,
                            ),
                      })
                    }
                    title={connection.name}
                    description={
                      connection.transport === "http"
                        ? "Remote MCP server"
                        : "Local MCP process"
                    }
                  />
                  {grant && (
                    <Field
                      label="Tool allowlist"
                      help="Leave empty for every tool from this connection. Use exact MCP tool names separated by commas."
                    >
                      <AllowlistInput
                        values={grant.tool_names}
                        onChange={(tool_names) =>
                          toolChange({
                            mcp: c.tools.mcp.map((g) =>
                              g.connection_id === connection.id
                                ? { ...g, tool_names }
                                : g,
                            ),
                          })
                        }
                      />
                    </Field>
                  )}
                </div>
              );
            })}
          </>
        )}
      </div>
      <footer>
        <Button icon="copy" tone="ghost" onClick={onDuplicate}>
          Duplicate
        </Button>
        <Button
          icon="trash"
          tone="danger"
          disabled={bp.agent_classes.length === 1}
          onClick={onDelete}
        >
          Delete class
        </Button>
      </footer>
    </aside>
  );
}
function Connections({ bp, update, setModal }) {
  const [query, setQuery] = useState(""),
    [results, setResults] = useState(null),
    [loading, setLoading] = useState(false),
    [error, setError] = useState("");
  async function search(value = query) {
    if (!value.trim()) return;
    setQuery(value);
    setLoading(true);
    setError("");
    try {
      setResults(
        await request(`/api/connectors/search?q=${encodeURIComponent(value)}`),
      );
    } catch (e) {
      setError(e.message);
    } finally {
      setLoading(false);
    }
  }
  return (
    <div className="section-scroll">
      <SectionHeading
        eyebrow="BRING YOUR WORLD INTO THE SYSTEM"
        title="Connections without the guesswork."
        description="Discover real MCP servers, inspect their source, and decide which agent classes can use each tool."
        action={
          <Button icon="plus" onClick={() => setModal({ type: "connector" })}>
            Custom connection
          </Button>
        }
      />
      <div className="registry-search">
        <div>
          <span className="registry-mark">
            <Icon name="link" size={25} />
          </span>
          <div>
            <h2>What should your agents connect to?</h2>
            <p>Search the official MCP Registry for tools and services.</p>
          </div>
          <Badge tone="green">Live registry</Badge>
        </div>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            search();
          }}
        >
          <Icon name="search" />
          <input
            aria-label="Search MCP Registry"
            placeholder="Try GitHub, Notion, web search, a database..."
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <Button tone="primary" disabled={loading || !query.trim()}>
            {loading ? "Searching..." : "Find connectors"}
          </Button>
        </form>
        <div className="search-suggestions">
          Explore{" "}
          {["GitHub", "Notion", "Postgres", "Brave search", "Linear"].map(
            (q) => (
              <button key={q} onClick={() => search(q)}>
                {q}
              </button>
            ),
          )}
        </div>
      </div>
      {error && (
        <div className="notice amber" role="alert">
          {error}
        </div>
      )}
      {results && (
        <section className="registry-results">
          <div className="subheading">
            <h2>Registry results</h2>
            <span>{results.results?.length || 0} servers</span>
            <button className="text-button" onClick={() => setResults(null)}>
              Clear
            </button>
          </div>
          {results.warnings?.map((w) => (
            <div key={w} className="notice amber">
              {w}
            </div>
          ))}
          {!results.results?.length ? (
            <Empty
              icon="search"
              title="No servers found"
              description="Try a service name or paste the provider's MCP configuration as a custom connection."
            />
          ) : (
            <div className="connection-grid">
              {results.results.map((r, i) => (
                <article
                  className="registry-card"
                  key={`${r.registry_id || r.name}-${i}`}
                >
                  <div className="registry-card-title">
                    <span className="connection-icon">
                      <Icon name="link" />
                    </span>
                    <div>
                      <h3>{r.name}</h3>
                      <small>{r.registry_id}</small>
                    </div>
                    <Badge>{r.version || "Registry"}</Badge>
                  </div>
                  <p>{r.description}</p>
                  {r.required_environment?.length > 0 && (
                    <div className="env-needed">
                      <Icon name="lock" size={13} />
                      {r.required_environment.length} local configuration values
                    </div>
                  )}
                  {r.warnings?.length > 0 && (
                    <details className="connection-notes">
                      <summary>Setup notes</summary>
                      {r.warnings.map((w) => (
                        <p key={w}>{w}</p>
                      ))}
                    </details>
                  )}
                  {!r.installable && (
                    <div className="notice amber">
                      {r.auth?.detail ||
                        "This server needs additional authentication or configuration. Open its source for setup details."}
                    </div>
                  )}
                  <footer>
                    {safeUrl(r.repository || r.source_url) ? (
                      <a
                        href={r.repository || r.source_url}
                        target="_blank"
                        rel="noreferrer"
                      >
                        View source
                        <Icon name="external" size={13} />
                      </a>
                    ) : (
                      <span />
                    )}
                    <Button
                      tone="secondary"
                      icon="plus"
                      disabled={!r.connection}
                      onClick={() => {
                        const connection = clone(r.connection);
                        connection.id = uniqueId(
                          connection.id,
                          bp.connections.map((c) => c.id),
                        );
                        setModal({ type: "connector", connection });
                      }}
                    >
                      Configure
                    </Button>
                  </footer>
                </article>
              ))}
            </div>
          )}
        </section>
      )}
      {bp.agent_classes.some((agent) => agent.tools.rust_bundles?.length) && (
        <section>
          <div className="subheading"><h2>Native bundle selections</h2></div>
          <div className="connection-grid">
            {bp.agent_classes.filter((agent) => agent.tools.rust_bundles?.length).map((agent) => (
              <article className="connected-card" key={agent.id}>
                <h3>{agent.name}</h3>
                <NativeBundleSelection tools={agent.tools} />
              </article>
            ))}
          </div>
        </section>
      )}
      <div className="subheading">
        <h2>Your connections</h2>
        <span>{bp.connections.length}</span>
      </div>
      {!bp.connections.length ? (
        <Empty
          icon="link"
          title="The right tools, for the right agents."
          description="Find a service above or add a custom MCP server. Connections only become available to the classes you choose."
        />
      ) : (
        <div className="connection-grid">
          {bp.connections.map((c) => {
            const users = bp.agent_classes.filter((a) =>
              a.tools.mcp.some((g) => g.connection_id === c.id),
            );
            return (
              <article className="connected-card" key={c.id}>
                <div className="registry-card-title">
                  <span className="connection-icon">
                    <Icon name={c.transport === "http" ? "globe" : "code"} />
                  </span>
                  <div>
                    <h3>{c.name}</h3>
                    <small>
                      {c.transport === "http" ? "Remote HTTP" : "Local process"}
                    </small>
                  </div>
                  <button
                    className="icon-button"
                    aria-label={`Edit ${c.name}`}
                    onClick={() =>
                      setModal({ type: "connector", connection: c })
                    }
                  >
                    <Icon name="settings" size={16} />
                  </button>
                </div>
                <code className="connection-address">
                  {c.transport === "http"
                    ? c.url
                    : `${c.command} ${c.args.join(" ")}`}
                </code>
                <div className="connection-users">
                  <span>AVAILABLE TO</span>
                  {users.length ? (
                    users.map((a) => <Badge key={a.id}>{a.name}</Badge>)
                  ) : (
                    <small>No classes assigned yet</small>
                  )}
                </div>
                <footer>
                  <button
                    className="text-button"
                    onClick={() => setModal({ type: "inspect", connection: c })}
                  >
                    <Icon name="search" size={13} /> Inspect tools
                  </button>
                  <button
                    className="icon-button danger"
                    aria-label={`Remove ${c.name}`}
                    onClick={() => update(removeConnection(bp, c.id))}
                  >
                    <Icon name="trash" size={15} />
                  </button>
                </footer>
              </article>
            );
          })}
        </div>
      )}
    </div>
  );
}
function safeUrl(url) {
  try {
    return ["http:", "https:"].includes(new URL(url).protocol);
  } catch {
    return false;
  }
}
function ConnectorModal({ bp, connection, onClose, onApply }) {
  const [mode, setMode] = useState(connection ? "form" : "paste"),
    [c, setC] = useState(() =>
      clone(
        connection || {
          id: uniqueId(
            "connection",
            bp.connections.map((c) => c.id),
          ),
          name: "My connection",
          transport: "stdio",
          command: "npx",
          args: [],
          url: "",
          env: {},
          headers: {},
          source: { registry_id: "", repository: "", version: "", url: "" },
        },
      ),
    );
  const [raw, setRaw] = useState(""),
    [args, setArgs] = useState((connection?.args || []).join("\n")),
    [env, setEnv] = useState(JSON.stringify(connection?.env || {}, null, 2)),
    [headers, setHeaders] = useState(
      JSON.stringify(connection?.headers || {}, null, 2),
    ),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  const edit = (patch) => setC((current) => ({ ...current, ...patch }));
  async function save() {
    setError("");
    setBusy(true);
    try {
      if (mode === "paste") {
        const result = await request("/api/connectors/import", {
          method: "POST",
          body: JSON.parse(raw),
        });
        if (result.connections?.length !== 1)
          throw new Error("Paste one server configuration at a time.");
        const next = result.connections[0];
        next.id = uniqueId(
          next.id,
          bp.connections.map((c) => c.id),
        );
        onApply(next);
      } else {
        const next = {
          ...c,
          args: c.transport === "stdio" ? args.split("\n").filter(Boolean) : [],
          command: c.transport === "stdio" ? c.command : "",
          env: c.transport === "stdio" ? JSON.parse(env) : {},
          url: c.transport === "http" ? c.url : "",
          headers: c.transport === "http" ? JSON.parse(headers) : {},
        };
        const draft = clone(bp),
          index = draft.connections.findIndex((conn) => conn.id === next.id);
        if (index >= 0) draft.connections[index] = next;
        else draft.connections.push(next);
        const normalized = await request("/api/blueprint/import", {
          method: "POST",
          body: draft,
        });
        onApply(
          normalized.blueprint.connections.find((conn) => conn.id === next.id),
        );
      }
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Modal
      wide
      title={
        connection ? `Configure ${connection.name}` : "Add an MCP connection"
      }
      subtitle="Keep credentials as environment references. The installer collects their values on your machine."
      onClose={onClose}
    >
      <div className="modal-body">
        <div className="segmented modal-tabs">
          <button
            className={mode === "paste" ? "active" : ""}
            onClick={() => setMode("paste")}
          >
            Paste MCP config
          </button>
          <button
            className={mode === "form" ? "active" : ""}
            onClick={() => setMode("form")}
          >
            Configure manually
          </button>
        </div>
        {mode === "paste" ? (
          <Field
            label="MCP configuration"
            help="Paste a standard mcpServers object or one MCP server. Secret values are replaced by local environment placeholders."
          >
            <textarea
              className="json-editor"
              rows={13}
              placeholder={
                '{\n  "mcpServers": {\n    "my-server": {\n      "command": "npx",\n      "args": ["-y", "the-official-package"]\n    }\n  }\n}'
              }
              value={raw}
              onChange={(e) => setRaw(e.target.value)}
            />
          </Field>
        ) : (
          <>
            <div className="field-grid">
              <Field label="Connection name">
                <input
                  value={c.name}
                  onChange={(e) => edit({ name: e.target.value })}
                />
              </Field>
              <Field label="Transport">
                <select
                  value={c.transport}
                  onChange={(e) => edit({ transport: e.target.value })}
                >
                  <option value="stdio">Local process (stdio)</option>
                  <option value="http">Remote HTTP</option>
                </select>
              </Field>
            </div>
            {c.transport === "stdio" ? (
              <>
                <Field label="Command">
                  <input
                    value={c.command}
                    onChange={(e) => edit({ command: e.target.value })}
                    placeholder="npx"
                  />
                </Field>
                <Field
                  label="Arguments"
                  help="One argument per line. No shell evaluation."
                >
                  <textarea
                    rows={4}
                    className="mono"
                    value={args}
                    onChange={(e) => setArgs(e.target.value)}
                    placeholder={"-y\n@provider/mcp-server@1.2.3"}
                  />
                </Field>
                <Field
                  label="Environment references (JSON)"
                  help={'Example: {"SERVICE_TOKEN": "${SERVICE_TOKEN}"}'}
                >
                  <textarea
                    rows={4}
                    className="mono"
                    value={env}
                    onChange={(e) => setEnv(e.target.value)}
                  />
                </Field>
              </>
            ) : (
              <>
                <Field label="MCP endpoint">
                  <input
                    value={c.url}
                    onChange={(e) => edit({ url: e.target.value })}
                    placeholder="https://service.example/mcp"
                  />
                </Field>
                <Field
                  label="HTTP headers (JSON)"
                  help={'Example: {"Authorization": "Bearer ${SERVICE_TOKEN}"}'}
                >
                  <textarea
                    rows={4}
                    className="mono"
                    value={headers}
                    onChange={(e) => setHeaders(e.target.value)}
                  />
                </Field>
              </>
            )}
            <details>
              <summary>Source and provenance</summary>
              <Field label="Repository">
                <input
                  value={c.source?.repository || ""}
                  onChange={(e) =>
                    edit({
                      source: { ...c.source, repository: e.target.value },
                    })
                  }
                />
              </Field>
              <Field label="Version">
                <input
                  value={c.source?.version || ""}
                  onChange={(e) =>
                    edit({ source: { ...c.source, version: e.target.value } })
                  }
                />
              </Field>
            </details>
          </>
        )}
        {error && (
          <div className="notice amber" role="alert">
            {error}
          </div>
        )}
      </div>
      <footer>
        <Button onClick={onClose}>Cancel</Button>
        <Button tone="primary" icon="check" disabled={busy} onClick={save}>
          {busy
            ? "Validating..."
            : connection
              ? "Save connection"
              : "Add connection"}
        </Button>
      </footer>
    </Modal>
  );
}
function Flows({ bp, update }) {
  const [selected, setSelected] = useState(bp.flows[0]?.id),
    [stepId, setStepId] = useState(null),
    [error, setError] = useState("");
  const flow = bp.flows.find((f) => f.id === selected) || bp.flows[0],
    step = flow?.steps.find((s) => s.id === stepId),
    members = identities(bp);
  function change(nextFlow) {
    update((next) => {
      next.flows = next.flows.map((f) => (f.id === nextFlow.id ? nextFlow : f));
      return next;
    });
  }
  function newFlow() {
    const id = uniqueId(
      "workflow",
      bp.flows.map((f) => f.id),
    );
    const f = {
      id,
      name: "New workflow",
      description: "A repeatable process for your team.",
      steps: [
        {
          id: "start",
          agent: bp.topology.entrypoint,
          prompt: "Review the request and prepare a clear plan of action.",
          depends_on: [],
          timeout_ms: 120000,
        },
      ],
    };
    update((next) => {
      next.flows.push(f);
      return next;
    });
    setSelected(id);
    setStepId("start");
  }
  function addStep() {
    const id = uniqueId(
      "step",
      flow.steps.map((s) => s.id),
    );
    change({
      ...flow,
      steps: [
        ...flow.steps,
        {
          id,
          agent: members[Math.min(flow.steps.length, members.length - 1)].id,
          prompt:
            "Complete your part of the task, using the preceding results as context.",
          depends_on: flow.steps.length ? [flow.steps.at(-1).id] : [],
          timeout_ms: 120000,
        },
      ],
    });
    setStepId(id);
  }
  function editStep(patch) {
    const next = {
      ...flow,
      steps: flow.steps.map((s) => (s.id === step.id ? { ...s, ...patch } : s)),
    };
    if (hasCycle(next.steps)) {
      setError(
        "That dependency would create a cycle. Choose a different preceding step.",
      );
      return;
    }
    setError("");
    change(next);
  }
  return (
    <div className="section-scroll">
      <SectionHeading
        eyebrow="TURN GOOD WORK INTO A REPEATABLE PROCESS"
        title="Give your team a rhythm."
        description="Build native MobKit workflows with parallel work, dependencies, and explicit handoffs."
        action={
          <Button icon="plus" onClick={newFlow}>
            New workflow
          </Button>
        }
      />
      {!flow ? (
        <Empty
          icon="flow"
          title="From a request to a reliable result."
          description="Create a workflow for research, reviews, planning, or any process your team repeats."
          action={
            <Button tone="primary" icon="plus" onClick={newFlow}>
              Create a workflow
            </Button>
          }
        />
      ) : (
        <>
          <div className="flow-tabs">
            {bp.flows.map((f) => (
              <button
                key={f.id}
                className={f.id === flow.id ? "active" : ""}
                onClick={() => {
                  setSelected(f.id);
                  setStepId(null);
                }}
              >
                <Icon name="flow" size={16} />
                {f.name}
                <Badge>{f.steps.length}</Badge>
              </button>
            ))}
          </div>
          <div className="flow-meta">
            <Field label="Workflow name">
              <input
                value={flow.name}
                onChange={(e) => change({ ...flow, name: e.target.value })}
              />
            </Field>
            <Field label="Purpose">
              <input
                value={flow.description}
                onChange={(e) =>
                  change({ ...flow, description: e.target.value })
                }
              />
            </Field>
            <button
              className="icon-button danger"
              aria-label="Delete workflow"
              onClick={() =>
                update((next) => {
                  next.flows = next.flows.filter((f) => f.id !== flow.id);
                  return next;
                })
              }
            >
              <Icon name="trash" />
            </button>
          </div>
          <div className="flow-workspace">
            <div className="flow-canvas">
              <div className="flow-start">
                <span />
                Manual trigger
              </div>
              <FlowDiagram
                flow={flow}
                bp={bp}
                selected={step?.id}
                onSelect={setStepId}
              />
              <Button icon="plus" onClick={addStep}>
                Add step
              </Button>
              <div className="flow-run-hint">
                <Icon name="code" size={16} />
                <span>
                  Run from the deployed console or{" "}
                  <code>bash start.sh --run-flow {flow.id}</code>
                </span>
              </div>
            </div>
            <aside className="flow-inspector">
              {step ? (
                <>
                  <div className="subheading">
                    <h3>Step: {step.id}</h3>
                    <button
                      className="icon-button danger"
                      disabled={flow.steps.length === 1}
                      aria-label="Delete step"
                      onClick={() => {
                        change(removeStep(flow, step.id));
                        setStepId(null);
                      }}
                    >
                      <Icon name="trash" size={16} />
                    </button>
                  </div>
                  <Field label="Assign to agent">
                    <select
                      value={step.agent}
                      onChange={(e) => editStep({ agent: e.target.value })}
                    >
                      {members.map((a) => (
                        <option key={a.id} value={a.id}>
                          {a.name} ({a.id})
                        </option>
                      ))}
                    </select>
                  </Field>
                  <Field label="Instructions">
                    <textarea
                      rows={7}
                      value={step.prompt}
                      onChange={(e) => editStep({ prompt: e.target.value })}
                    />
                  </Field>
                  <Field
                    label="Wait for these steps"
                    help="Without dependencies, this step can run immediately."
                  >
                    <div className="dependency-options">
                      {flow.steps
                        .filter((s) => s.id !== step.id)
                        .map((s) => (
                          <label key={s.id}>
                            <input
                              type="checkbox"
                              checked={step.depends_on.includes(s.id)}
                              onChange={(e) =>
                                editStep({
                                  depends_on: e.target.checked
                                    ? [...step.depends_on, s.id]
                                    : step.depends_on.filter(
                                        (id) => id !== s.id,
                                      ),
                                })
                              }
                            />
                            {s.id}
                          </label>
                        ))}
                      {flow.steps.length === 1 && (
                        <small>Add another step to define dependencies.</small>
                      )}
                    </div>
                  </Field>
                  <Field label="Timeout (seconds)">
                    <input
                      type="number"
                      min="1"
                      max="1800"
                      value={step.timeout_ms / 1000}
                      onChange={(e) =>
                        editStep({
                          timeout_ms: Math.max(
                            1000,
                            Number(e.target.value) * 1000,
                          ),
                        })
                      }
                    />
                  </Field>
                  {error && <div className="notice amber">{error}</div>}
                </>
              ) : (
                <Empty
                  icon="flow"
                  title="Shape each step"
                  description="Select a step to choose its agent, instructions, and dependencies."
                />
              )}
            </aside>
          </div>
        </>
      )}
    </div>
  );
}
function FlowDiagram({ flow, bp, selected, onSelect }) {
  const members = identities(bp),
    layout = flowLayout(flow.steps),
    byId = Object.fromEntries(layout.nodes.map((n) => [n.step.id, n]));
  return (
    <div className="flow-diagram-scroll">
      <div
        className="flow-diagram"
        style={{ width: layout.width, height: layout.height }}
      >
        <svg
          className="flow-diagram-lines"
          viewBox={`0 0 ${layout.width} ${layout.height}`}
          aria-hidden="true"
        >
          <defs>
            <marker
              id="flow-arrow"
              markerWidth="6"
              markerHeight="6"
              refX="5"
              refY="3"
              orient="auto"
            >
              <path d="M0 0L6 3L0 6" fill="none" stroke="#a9bd93" />
            </marker>
          </defs>
          {layout.nodes.flatMap((n) =>
            n.step.depends_on.map((id) => {
              const from = byId[id];
              if (!from) return null;
              const x1 = from.x + 105,
                x2 = n.x + 105,
                y1 = from.y + 62,
                y2 = n.y,
                middle = (y1 + y2) / 2;
              return (
                <path
                  key={`${id}-${n.step.id}`}
                  d={`M${x1} ${y1}V${middle}H${x2}V${y2 - 4}`}
                  fill="none"
                  stroke="#b4c8a0"
                  strokeWidth="1.4"
                  markerEnd="url(#flow-arrow)"
                />
              );
            }),
          )}
        </svg>
        {layout.nodes.map((n) => {
          const member = members.find((a) => a.id === n.step.agent);
          return (
            <button
              key={n.step.id}
              className={`flow-step ${selected === n.step.id ? "selected" : ""}`}
              style={{ left: n.x, top: n.y }}
              onClick={() => onSelect(n.step.id)}
            >
              <Avatar
                label={member?.name || n.step.agent}
                index={bp.agent_classes.findIndex(
                  (c) => c.id === member?.classId,
                )}
              />
              <div>
                <strong>{n.step.id.replaceAll("-", " ")}</strong>
                <small>{member?.name || n.step.agent}</small>
              </div>
              <span className="flow-step-time">
                {Math.round(n.step.timeout_ms / 1000)}s
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
function Knowledge({ bp, update }) {
  const [capabilities, setCapabilities] = useState(null);
  useEffect(() => {
    request("/api/knowledge/capabilities")
      .then(setCapabilities)
      .catch(() => {});
  }, []);
  const k = bp.knowledge,
    change = (patch) =>
      update((next) => {
        Object.assign(next.knowledge, patch);
        if (patch.mode === "local" && next.target === "gcp") {
          if (
            !["e2-standard-8", "e2-standard-16"].includes(next.gcp.machine_type)
          )
            next.gcp.machine_type = "e2-standard-8";
          next.gcp.disk_gb = Math.max(100, next.gcp.disk_gb || 30);
        }
        if (patch.mode === "none")
          next.agent_classes.forEach((c) => {
            c.tools.knowledge = false;
          });
        return next;
      });
  return (
    <div className="section-scroll">
      <SectionHeading
        eyebrow="KNOWLEDGE THAT OUTLIVES A CONVERSATION"
        title="A shared understanding of your world."
        description="Add a knowledge service for structured, persistent knowledge. Choose an existing space or a private deployment alongside your agents."
      />
      <div className="knowledge-hero">
        <div className="elephant-mark">
          <Icon name="database" size={38} />
        </div>
        <div>
          <Badge tone="purple">KNOWLEDGE SERVICE</Badge>
          <h2>
            Connect the dots.
            <br />
            Keep the context.
          </h2>
          <p>
            A durable knowledge graph, scoped to your project. Agents get read
            access to the space you choose.
          </p>
        </div>
        <div className="knowledge-orbit">
          <span>sources</span>
          <span>entities</span>
          <span>relationships</span>
          <Icon name="database" size={34} />
        </div>
      </div>
      <div className="knowledge-modes">
        {[
          {
            id: "none",
            title: "No knowledge base",
            description: "Start with agent memory and shared SQLite notes.",
            icon: "folder",
          },
          {
            id: "remote",
            title: "Connect to a knowledge service",
            description: "Use an existing knowledge service MCP endpoint and space.",
            icon: "globe",
          },
          {
            id: "local",
            title: "Deploy your own",
            description: "Create a private knowledge service with SurrealDB.",
            icon: "database",
          },
        ].map((mode) => (
          <button
            key={mode.id}
            className={k.mode === mode.id ? "active" : ""}
            aria-label={mode.title}
            onClick={() => change({ mode: mode.id })}
          >
            <Icon name={mode.icon} size={24} />
            <strong>{mode.title}</strong>
            <p>{mode.description}</p>
            <span className="radio-dot" />
          </button>
        ))}
      </div>
      {k.mode === "local" && capabilities && (
        <div className="knowledge-requirements">
          <span className="eyebrow">LOCAL REQUIREMENTS</span>
          <div>
            {capabilities.local_requirements.map((item) => (
              <Badge key={item}>{item}</Badge>
            ))}
          </div>
        </div>
      )}
      {k.mode !== "none" && (
        <div className="knowledge-config">
          <div>
            <div className="subheading">
              <h2>
                {k.mode === "local"
                  ? "Private deployment"
                  : "Connection settings"}
              </h2>
              <Badge>
                {k.mode === "local" ? "Docker Compose" : "Remote MCP"}
              </Badge>
            </div>
            <Field label="Agent MCP endpoint">
              <input
                value={k.url}
                onChange={(e) => change({ url: e.target.value })}
              />
            </Field>
            <div className="field-grid">
              <Field label="Token environment name">
                <input
                  value={k.token_env}
                  onChange={(e) => change({ token_env: e.target.value })}
                />
              </Field>
              <Field label="Space environment name">
                <input
                  value={k.space_env}
                  onChange={(e) => change({ space_env: e.target.value })}
                />
              </Field>
            </div>
            {k.mode === "local" && (
              <>
                <div className="notice">
                  <Icon name="lock" />
                  <span>
                    The package provisions an isolated, persistent database and
                    a loopback-only API. Existing knowledge services are
                    untouched.
                  </span>
                </div>
                <details>
                  <summary>Source pin and build</summary>
                  <Field
                    label="Source repository"
                    help="Leave blank to use the verified package default."
                  >
                    <input
                      value={k.source_repository}
                      placeholder={capabilities?.repository || ""}
                      onChange={(e) =>
                        change({ source_repository: e.target.value })
                      }
                    />
                  </Field>
                  <Field
                    label="Source revision"
                    help="The generated package records its pinned source revision."
                  >
                    <input
                      value={k.source_revision}
                      placeholder={capabilities?.revision || ""}
                      onChange={(e) =>
                        change({ source_revision: e.target.value })
                      }
                    />
                  </Field>
                </details>
              </>
            )}
          </div>
          <div className="knowledge-grants">
            <span className="eyebrow">KNOWLEDGE ACCESS</span>
            <h3>Who can use this space?</h3>
            <p>Only the classes you select receive the knowledge connection.</p>
            {bp.agent_classes.map((c) => (
              <Toggle
                key={c.id}
                value={c.tools.knowledge}
                title={c.name}
                description={`${c.instances} ${c.instances === 1 ? "agent" : "agents"}`}
                onChange={(v) =>
                  update((next) => {
                    next.agent_classes.find(
                      (a) => a.id === c.id,
                    ).tools.knowledge = v;
                    return next;
                  })
                }
              />
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
function Deployment({
  bp,
  update,
  config,
  validation,
  validating,
  validate,
  download,
}) {
  const [selected, setSelected] = useState("");
  const files = validation?.files || [],
    chosen = files.find((f) => f.path === selected) || files[0];
  return (
    <div className="section-scroll">
      <SectionHeading
        eyebrow="YOUR SYSTEM, ON YOUR TERMS"
        title="From blueprint to running team."
        description="Choose where your system lives. Inspect the exact package, then run the installer on your machine."
      />
      <div className="deployment-targets">
        {[
          {
            id: "mac",
            title: "This Mac",
            subtitle: "Local macOS",
            icon: "monitor",
          },
          {
            id: "linux",
            title: "Linux machine",
            subtitle: "Local or your own server",
            icon: "code",
          },
          {
            id: "gcp",
            title: "Google Cloud",
            subtitle: "Private VM with IAP access",
            icon: "cloud",
          },
        ].map((target) => (
          <button
            className={bp.target === target.id ? "active" : ""}
            key={target.id}
            aria-label={target.title}
            onClick={() =>
              update((next) => {
                next.target = target.id;
                if (target.id === "gcp" && next.knowledge.mode === "local") {
                  if (
                    !["e2-standard-8", "e2-standard-16"].includes(
                      next.gcp.machine_type,
                    )
                  )
                    next.gcp.machine_type = "e2-standard-8";
                  next.gcp.disk_gb = Math.max(100, next.gcp.disk_gb || 30);
                }
                return next;
              })
            }
          >
            <Icon name={target.icon} size={26} />
            <strong>{target.title}</strong>
            <small>{target.subtitle}</small>
            <span className="radio-dot" />
          </button>
        ))}
      </div>
      <div className="deployment-config">
        <div>
          <Field label="Project directory">
            <div className="input-prefix">
              <span>~/src/</span>
              <input
                value={bp.project.slug}
                onChange={(e) =>
                  update((next) => {
                    next.project.slug = e.target.value;
                    return next;
                  })
                }
              />
            </div>
          </Field>
          <Field label="Console port">
            <input
              type="number"
              min="1024"
              max="65535"
              value={bp.advanced.port}
              onChange={(e) =>
                update((next) => {
                  next.advanced.port = Number(e.target.value);
                  return next;
                })
              }
            />
          </Field>
          {bp.target === "gcp" && (
            <>
              <Field label="Google Cloud project ID">
                <input
                  value={bp.gcp.project}
                  placeholder={
                    config?.capabilities?.gcp?.project || "your-project-id"
                  }
                  onChange={(e) =>
                    update((next) => {
                      next.gcp.project = e.target.value;
                      return next;
                    })
                  }
                />
              </Field>
              <div className="field-grid">
                <Field label="Zone">
                  <input
                    value={bp.gcp.zone}
                    onChange={(e) =>
                      update((next) => {
                        next.gcp.zone = e.target.value;
                        next.gcp.region = e.target.value.replace(/-[a-z]$/, "");
                        return next;
                      })
                    }
                  />
                </Field>
                <Field label="Machine">
                  <select
                    value={bp.gcp.machine_type}
                    onChange={(e) =>
                      update((next) => {
                        next.gcp.machine_type = e.target.value;
                        return next;
                      })
                    }
                  >
                    <option value="e2-small">e2-small</option>
                    <option value="e2-medium">e2-medium</option>
                    <option value="e2-standard-2">e2-standard-2</option>
                    <option value="e2-standard-4">e2-standard-4</option>
                    <option value="e2-standard-8">e2-standard-8</option>
                    <option value="e2-standard-16">e2-standard-16</option>
                  </select>
                </Field>
              </div>
              <Field label="Boot disk (GB)">
                <input
                  type="number"
                  min="30"
                  max="2000"
                  value={bp.gcp.disk_gb || 30}
                  onChange={(e) =>
                    update((next) => {
                      next.gcp.disk_gb = Number(e.target.value);
                      return next;
                    })
                  }
                />
              </Field>
              {bp.knowledge.mode === "local" && (
                <div className="notice">
                  The local knowledge service build requires at least e2-standard-8 and
                  100 GB. These have been selected for this package.
                </div>
              )}
              <div className="notice amber">
                The generated script confirms your cloud project before creating
                billable resources.
              </div>
            </>
          )}
        </div>
        <div className="deployment-summary">
          <span className="eyebrow">IN YOUR PACKAGE</span>
          {[
            [
              `${identities(bp).length} durable agents`,
              "Native identity roster and model configuration",
            ],
            [
              `${bp.connections.length} MCP connections`,
              "Per-class tool grants and local credential setup",
            ],
            [
              `${bp.flows.length} workflows`,
              "Native flow definitions with dependencies",
            ],
            [
              bp.knowledge.mode === "none"
                ? "Persistent local memory"
                : "Knowledge service",
              bp.knowledge.mode === "local"
                ? "Isolated knowledge service + SurrealDB deployment"
                : "Shared SQLite notes and agent continuity",
            ],
            [
              "Your own MobKit console",
              "Inspect, message, and operate your running team",
            ],
          ].map(([title, sub]) => (
            <div key={title}>
              <Icon name="check" size={17} />
              <span>
                <strong>{title}</strong>
                <small>{sub}</small>
              </span>
            </div>
          ))}
        </div>
      </div>
      <div className="build-bar">
        <div>
          <strong>Ready to make it real?</strong>
          <p>
            Validate the complete blueprint and inspect every generated file.
          </p>
        </div>
        <div className="button-row">
          <Button icon="code" disabled={validating} onClick={validate}>
            {validating ? "Validating..." : "Review package"}
          </Button>
          <Button
            icon="download"
            tone="primary"
            disabled={validating}
            onClick={download}
          >
            Download deployment
          </Button>
        </div>
      </div>
      {validation && (
        <section className="package-preview">
          <div className="subheading">
            <h2>Deployment package</h2>
            <Badge tone="green">Validated</Badge>
            <span>{files.length} files</span>
          </div>
          {validation.checks?.length > 0 && (
            <div className="validation-checks">
              {validation.checks.map((check, i) => (
                <span key={i}>
                  <Icon name="check" size={14} />
                  {typeof check === "string"
                    ? check
                    : check.message || check.label || check.name}
                </span>
              ))}
            </div>
          )}
          <div className="file-browser">
            <nav>
              {files.map((file) => (
                <button
                  key={file.path}
                  className={chosen?.path === file.path ? "active" : ""}
                  onClick={() => setSelected(file.path)}
                >
                  <Icon name="file" size={14} />
                  {file.path}
                </button>
              ))}
            </nav>
            <div>
              <header>
                <code>{chosen?.path}</code>
                <button
                  className="text-button"
                  onClick={() =>
                    navigator.clipboard.writeText(chosen?.content || "")
                  }
                >
                  <Icon name="copy" size={14} /> Copy
                </button>
              </header>
              <pre>{chosen?.content}</pre>
            </div>
          </div>
          <div className="run-command">
            <span>After downloading and unzipping</span>
            <code>
              {bp.target === "gcp"
                ? "bash deploy/deploy-gcp.sh"
                : "bash install.sh && bash start.sh"}
            </code>
          </div>
        </section>
      )}
    </div>
  );
}
function ProjectModal({ bp, onApply, onClose }) {
  const [draft, setDraft] = useState(clone(bp));
  return (
    <Modal
      title="Your project"
      subtitle="The purpose that brings your agents together."
      onClose={onClose}
    >
      <div className="modal-body">
        <Field label="Project name">
          <input
            value={draft.project.name}
            maxLength={80}
            onChange={(e) =>
              setDraft({
                ...draft,
                project: { ...draft.project, name: e.target.value },
              })
            }
          />
        </Field>
        <Field label="Purpose">
          <textarea
            rows={5}
            value={draft.project.description}
            onChange={(e) =>
              setDraft({
                ...draft,
                project: { ...draft.project, description: e.target.value },
              })
            }
          />
        </Field>
        <Field
          label="Default provider"
          help="Used for new classes. The setup architect uses the server's configured provider."
        >
          <select
            value={draft.provider}
            onChange={(e) =>
              setDraft({
                ...draft,
                provider: e.target.value,
                model: DEFAULT_MODELS[e.target.value],
              })
            }
          >
            <option value="openai">OpenAI</option>
            <option value="anthropic">Anthropic</option>
            <option value="gemini">Google Gemini</option>
          </select>
        </Field>
        <Field label="Default model">
          <input
            value={draft.model}
            onChange={(e) => setDraft({ ...draft, model: e.target.value })}
          />
        </Field>
      </div>
      <footer>
        <Button onClick={onClose}>Cancel</Button>
        <Button tone="primary" onClick={() => onApply(draft)}>
          Save project
        </Button>
      </footer>
    </Modal>
  );
}
function JsonModal({ bp, onApply, onClose }) {
  const [raw, setRaw] = useState(JSON.stringify(bp, null, 2)),
    [error, setError] = useState(""),
    [busy, setBusy] = useState(false);
  async function save() {
    setBusy(true);
    setError("");
    try {
      onApply(
        (
          await request("/api/blueprint/import", {
            method: "POST",
            body: JSON.parse(raw),
          })
        ).blueprint,
      );
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Modal
      wide
      title="Blueprint source"
      subtitle="Edit the complete configuration. It is validated before being applied."
      onClose={onClose}
    >
      <div className="modal-body">
        <textarea
          aria-label="Blueprint JSON"
          className="json-editor"
          rows={24}
          value={raw}
          onChange={(e) => setRaw(e.target.value)}
        />
        {error && <div className="notice amber">{error}</div>}
      </div>
      <footer>
        <Button onClick={onClose}>Cancel</Button>
        <Button tone="primary" disabled={busy} onClick={save}>
          {busy ? "Validating..." : "Apply blueprint"}
        </Button>
      </footer>
    </Modal>
  );
}
function EdgeModal({ bp, onClose, onApply }) {
  const [edges, setEdges] = useState(clone(resolvedEdges(bp))),
    [a, setA] = useState(identities(bp)[0]?.id),
    [b, setB] = useState(identities(bp)[1]?.id || identities(bp)[0]?.id);
  const members = identities(bp),
    names = Object.fromEntries(members.map((m) => [m.id, m.name]));
  const duplicate = edges.some(
    (e) => [e.a, e.b].includes(a) && [e.a, e.b].includes(b),
  );
  return (
    <Modal
      wide
      title="Shape the communication topology"
      subtitle="Each link lets two agent identities communicate. Saving switches to a custom topology."
      onClose={onClose}
    >
      <div className="modal-body">
        <div className="edge-builder">
          <Field label="Agent">
            <select value={a} onChange={(e) => setA(e.target.value)}>
              {members.map((m) => (
                <option value={m.id} key={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
          </Field>
          <Icon name="link" />
          <Field label="Peer">
            <select value={b} onChange={(e) => setB(e.target.value)}>
              {members.map((m) => (
                <option value={m.id} key={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
          </Field>
          <Button
            icon="plus"
            disabled={a === b || duplicate}
            onClick={() => setEdges([...edges, { a, b }])}
          >
            Add link
          </Button>
        </div>
        <div className="edge-list">
          {edges.map((e, i) => (
            <div key={`${e.a}-${e.b}`}>
              <span>
                {names[e.a]}
                <code>{e.a}</code>
              </span>
              <Icon name="link" size={16} />
              <span>
                {names[e.b]}
                <code>{e.b}</code>
              </span>
              <button
                className="icon-button danger"
                aria-label={`Remove link ${e.a} to ${e.b}`}
                onClick={() =>
                  setEdges(edges.filter((_, index) => index !== i))
                }
              >
                <Icon name="close" size={16} />
              </button>
            </div>
          ))}
          {!edges.length && (
            <p className="help-text">
              No links configured. These agents will work independently.
            </p>
          )}
        </div>
      </div>
      <footer>
        <span className="help-text">{edges.length} peer links</span>
        <Button onClick={onClose}>Cancel</Button>
        <Button tone="primary" icon="check" onClick={() => onApply(edges)}>
          Save topology
        </Button>
      </footer>
    </Modal>
  );
}
function InspectToolsModal({ connection, bp, update, onClose }) {
  const [result, setResult] = useState(null),
    [busy, setBusy] = useState(false),
    [error, setError] = useState(""),
    [selected, setSelected] = useState(bp.agent_classes[0].id);
  const agent = bp.agent_classes.find((c) => c.id === selected),
    grant = agent.tools.mcp.find((g) => g.connection_id === connection.id);
  async function inspect() {
    setBusy(true);
    setResult(null);
    setError("");
    try {
      setResult(
        await request("/api/connectors/inspect", {
          method: "POST",
          body: { connection },
        }),
      );
    } catch (e) {
      setError(e.message);
    } finally {
      setBusy(false);
    }
  }
  function setGrant(names) {
    update((next) => {
      const c = next.agent_classes.find((a) => a.id === selected);
      c.tools.mcp = c.tools.mcp.filter(
        (g) => g.connection_id !== connection.id,
      );
      if (names !== null)
        c.tools.mcp.push({ connection_id: connection.id, tool_names: names });
      return next;
    });
  }
  const selectedNames = grant
    ? grant.tool_names.length
      ? grant.tool_names
      : (result?.tools || []).map((t) => t.name)
    : [];
  return (
    <Modal
      wide
      title={`Inspect ${connection.name}`}
      subtitle="Read the actual tools exposed by this MCP server, then assign exact tool grants to your classes."
      onClose={onClose}
    >
      <div className="modal-body">
        {!result && (
          <>
            <div className="notice amber">
              <Icon name="info" />
              <span>
                {connection.transport === "stdio"
                  ? "This starts the configured MCP process on the studio machine to list its tools."
                  : "This contacts the configured remote MCP endpoint to list its tools."}{" "}
                It uses credentials from the studio server environment. It does
                not call any listed tools.
              </span>
            </div>
            <code className="inspect-command">
              {connection.transport === "stdio"
                ? `${connection.command} ${connection.args.join(" ")}`
                : connection.url}
            </code>
            <Button
              icon="search"
              tone="primary"
              disabled={busy}
              onClick={inspect}
            >
              {busy
                ? "Connecting to MCP server..."
                : "Inspect tools on this machine"}
            </Button>
          </>
        )}
        {error && (
          <div className="notice amber" role="alert">
            {error}
          </div>
        )}
        {result && (
          <>
            <div className="inspection-header">
              <Badge tone="green">Handshake succeeded</Badge>
              <span>{result.tool_count} tools discovered</span>
              <Button
                tone="ghost"
                icon="search"
                disabled={busy}
                onClick={inspect}
              >
                Refresh
              </Button>
            </div>
            <Field label="Assign tools to class">
              <select
                value={selected}
                onChange={(e) => setSelected(e.target.value)}
              >
                {bp.agent_classes.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </select>
            </Field>
            <div className="inspection-actions">
              <Button tone="secondary" onClick={() => setGrant([])}>
                Allow all tools
              </Button>
              <Button tone="ghost" onClick={() => setGrant(null)}>
                Remove access
              </Button>
              <span>
                {!grant
                  ? "No access"
                  : grant.tool_names.length
                    ? `${grant.tool_names.length} tools selected`
                    : "All tools, including future tools"}
              </span>
            </div>
            <div className="inspected-tools">
              {result.tools.map((tool) => (
                <div key={tool.name}>
                  <label>
                    <input
                      type="checkbox"
                      checked={selectedNames.includes(tool.name)}
                      onChange={(e) => {
                        const names = e.target.checked
                          ? [...new Set([...selectedNames, tool.name])]
                          : selectedNames.filter((name) => name !== tool.name);
                        setGrant(names.length ? names : null);
                      }}
                    />
                    <span>
                      <strong>{tool.name}</strong>
                      <p>{tool.description}</p>
                    </span>
                    {tool.read_only === true && (
                      <Badge tone="green">Read only</Badge>
                    )}
                    {tool.destructive === true && (
                      <Badge>Can change data</Badge>
                    )}
                  </label>
                  <details>
                    <summary>Input schema</summary>
                    <pre className="code-block">
                      {JSON.stringify(tool.input_schema, null, 2)}
                    </pre>
                  </details>
                </div>
              ))}
            </div>
          </>
        )}
      </div>
      <footer>
        <Button onClick={onClose}>Done</Button>
      </footer>
    </Modal>
  );
}
function ProjectsModal({ onClose, onLoad }) {
  const [projects, setProjects] = useState([]),
    [selected, setSelected] = useState(null),
    [revisions, setRevisions] = useState([]),
    [error, setError] = useState(""),
    [loading, setLoading] = useState(true);
  const lifetime = useRef({ mounted: false, project: null, restore: 0 });
  function selectProject(project) {
    if (lifetime.current.project === (project?.slug ?? null)) return;
    lifetime.current.project = project?.slug ?? null;
    lifetime.current.restore += 1;
    setSelected(project);
    setRevisions([]);
    setError("");
  }
  function close() {
    lifetime.current.mounted = false;
    lifetime.current.restore += 1;
    onClose();
  }
  useEffect(() => {
    lifetime.current.mounted = true;
    let active = true;
    request("/api/projects")
      .then((result) => {
        if (!active || !lifetime.current.mounted) return;
        setProjects(result.projects);
        selectProject(result.projects[0] || null);
      })
      .catch((e) => { if (active && lifetime.current.mounted) setError(e.message); })
      .finally(() => { if (active && lifetime.current.mounted) setLoading(false); });
    return () => {
      active = false;
      lifetime.current.mounted = false;
      lifetime.current.restore += 1;
    };
  }, []);
  useEffect(() => {
    if (!selected) return;
    let active = true;
    const current = () => active && lifetime.current.mounted && lifetime.current.project === selected.slug;
    request(`/api/projects/${selected.slug}/history`)
      .then((result) => {
        if (current()) setRevisions(result.revisions);
      })
      .catch((e) => { if (current()) setError(e.message); });
    return () => { active = false; };
  }, [selected]);
  async function loadRevision(revision) {
    const owner = lifetime.current, project = selected;
    if (!owner.mounted || !project || owner.project !== project.slug) return;
    const ticket = ++owner.restore;
    const current = () => owner.mounted && owner.project === project.slug && owner.restore === ticket;
    setError("");
    try {
      const result = await request(`/api/projects/${project.slug}?revision=${revision}`);
      if (!current()) return;
      owner.restore += 1;
      onLoad(result.blueprint, project.revision);
    } catch (e) {
      if (current()) setError(e.message);
    }
  }
  return (
    <Modal
      wide
      title="Projects and revision history"
      subtitle="Named project revisions are saved by the local Meta Forge server. Loading a revision replaces your current draft; Undo brings it back."
      onClose={close}
    >
      <div className="modal-body">
        {error && <div className="notice amber">{error}</div>}
        {loading ? (
          <p className="help-text">Loading projects...</p>
        ) : !projects.length ? (
          <Empty
            icon="folder"
            title="Save your first revision"
            description="Use Save revision in the header to keep a named, restorable copy of your architecture. Your draft is already saved on this device."
          />
        ) : (
          <div className="project-revisions">
            <div className="saved-projects">
              {projects.map((p) => (
                <button
                  key={p.slug}
                  className={selected?.slug === p.slug ? "active" : ""}
                  onClick={() => selectProject(p)}
                >
                  <Icon name="folder" />
                  <span>
                    <strong>{p.name}</strong>
                    <small>{p.slug}</small>
                  </span>
                  <Badge>r{p.revision}</Badge>
                </button>
              ))}
            </div>
            <div className="revision-list">
              <span className="eyebrow">REVISION HISTORY</span>
              {revisions.map((r) => (
                <div key={r.revision}>
                  <Icon name="file" size={16} />
                  <span>
                    Revision {r.revision}
                    <small>{new Date(r.updated_at).toLocaleString()}</small>
                  </span>
                  <Button onClick={() => loadRevision(r.revision)}>
                    {r.revision === selected?.revision ? "Open" : "Restore"}
                  </Button>
                </div>
              ))}
            </div>
          </div>
        )}
      </div>
      <footer>
        <Button onClick={close}>Close</Button>
      </footer>
    </Modal>
  );
}
const appRoot = typeof document === "undefined" ? null : document.getElementById("app");
if (appRoot) createRoot(appRoot).render(<App />);
