import React from "react";

import {
  EMPTY_NEW_AGENT_DRAFT,
  agentChangeConfirmWords,
  agentRefRequest,
  agentShownName,
  archivedAgentControls,
  consentWords,
  createAgentsRequest,
  editDraftOf,
  outcomeWords,
  refusalFormField,
  refusalSentence,
  requestIdFor,
  requesterWords,
  updateAgentRequest,
  verdictWords,
  type AgentEditDraft,
  type AgentIdentityControls,
  type AgentRecord,
  type AgentSummary,
  type DirectoryVerdict,
  type IdentityChangeKind,
  type IdentityFormField,
  type IdentityRefusal,
  type NewAgentDraft,
  type RequestIdSlot,
} from "../lib/identity-changes";
import type { IdentityChangeResult, IdentityChangesApi, IdentityDirectory } from "../lib/use-identity-changes";
import { errorMessage } from "../lib/errors";
import { principalName } from "./decision-cards";
import {
  AgentName,
  Button,
  Countdown,
  KeyValueList,
  LoadingState,
  Notice,
  StatusMark,
  StatusPill,
  TechnicalDetails,
  cx,
  formatAbsoluteTime,
  humanizeEnum,
  type Tone,
} from "../../../packages/console-components/src/panel-kit";

type FieldErrors = Partial<Record<IdentityFormField, string>>;

/** What the notice under a form or a confirmation shows. A note is the
 * console's own word: that nothing is changed yet, or (an alert) that the
 * agent's record could not be read. */
type ShownResult = IdentityChangeResult | { kind: "withdrawn" } | { kind: "note"; message: string; alert?: boolean };

/** A field of a form: its label, its control, a hint, and the refusal that
 * names it. */
function Field({
  field,
  label,
  hint,
  error,
  children,
}: {
  field: IdentityFormField;
  label: string;
  hint?: React.ReactNode;
  error?: string;
  children: (ids: { describedBy?: string; invalid: boolean }) => React.ReactNode;
}) {
  const errorId = React.useId();
  return (
    <div className="identity-form__field" data-field={field} data-invalid={error ? "true" : undefined}>
      <span className="identity-form__label">{label}</span>
      {children({ describedBy: error ? errorId : undefined, invalid: Boolean(error) })}
      {hint ? <span className="identity-form__hint">{hint}</span> : null}
      {error ? (
        <span className="identity-form__error" id={errorId} role="alert" data-testid={`identity-field-error:${field}`}>
          {error}
        </span>
      ) : null}
    </div>
  );
}

const VERDICT_TONE: Record<string, Tone> = { ok: "ok", warn: "warn", crit: "crit", neutral: "neutral" };

/** The verdict of a directory entry: a pill, and the decision's lines or
 * the rule's reason under it. */
function Verdict({ entry }: { entry: DirectoryVerdict }) {
  const words = verdictWords(entry);
  return (
    <>
      <StatusPill tone={VERDICT_TONE[words.tone]} variant="quiet">{words.label}</StatusPill>
      {words.detail.length ? (
        <span className="identity-form__verdict-detail">
          {words.detail.map((line, index) => <span key={index}>{line}</span>)}
        </span>
      ) : null}
    </>
  );
}

/** The profiles of the directory, one to pick, each with its verdict. */
function ProfilePicker({
  directory,
  value,
  onChange,
  describedBy,
  invalid,
}: {
  directory: IdentityDirectory;
  value: string;
  onChange: (profile: string) => void;
  describedBy?: string;
  invalid: boolean;
}) {
  const name = React.useId();
  // A profile the record names that the directory does not list stays
  // shown, so the form never changes what it was opened on by itself.
  const listed = directory.profiles.some((entry) => entry.profile === value) || !value
    ? directory.profiles
    : [{ profile: value, verdict: "", lines: [] }, ...directory.profiles];
  return (
    <div className="identity-form__options" role="radiogroup" aria-label="Profile" aria-describedby={describedBy} aria-invalid={invalid || undefined} data-testid="identity-field:profile">
      {listed.map((entry) => {
        const words = verdictWords(entry);
        return (
          <label key={entry.profile} className={cx("identity-form__option", !words.pickable && "is-refused")}>
            <input
              type="radio"
              name={name}
              value={entry.profile}
              checked={value === entry.profile}
              disabled={!words.pickable}
              onChange={() => onChange(entry.profile)}
              data-testid={`identity-profile:${entry.profile}`}
            />
            <span className="identity-form__option-name">{humanizeEnum(entry.profile)}</span>
            {entry.verdict ? <Verdict entry={entry} /> : null}
          </label>
        );
      })}
      {listed.length === 0 ? <span className="identity-form__hint">This team lists no profile.</span> : null}
    </div>
  );
}

/** The agents of the directory, any number to pick, each with its verdict. */
function PeerPicker({
  directory,
  value,
  exclude,
  onChange,
  describedBy,
}: {
  directory: IdentityDirectory;
  value: string[];
  /** The agent the form edits: it is no peer of itself. */
  exclude?: string;
  onChange: (peers: string[]) => void;
  describedBy?: string;
}) {
  const listed = directory.peers.filter((peer) => peer.name !== exclude);
  // A peer the record names that the directory does not list stays shown.
  const unlisted = value.filter((peer) => !listed.some((entry) => entry.name === peer));
  const toggle = (peer: string, checked: boolean) =>
    onChange(checked ? [...value, peer] : value.filter((entry) => entry !== peer));
  return (
    <div className="identity-form__options" role="group" aria-label="Peers" aria-describedby={describedBy} data-testid="identity-field:peers">
      {listed.map((peer) => {
        const words = verdictWords(peer);
        const checked = value.includes(peer.name);
        return (
          <label key={peer.name} className={cx("identity-form__option", !words.pickable && "is-refused")}>
            <input
              type="checkbox"
              checked={checked}
              // A peer the agent already has can always be taken away.
              disabled={!words.pickable && !checked}
              onChange={(event) => toggle(peer.name, event.target.checked)}
              data-testid={`identity-peer:${peer.name}`}
            />
            <span className="identity-form__option-name" title={peer.name}>{agentShownName(peer)}</span>
            <span className="identity-form__option-meta">{humanizeEnum(peer.profile)}</span>
            {peer.verdict !== "allowed" ? <Verdict entry={peer} /> : null}
          </label>
        );
      })}
      {unlisted.map((peer) => (
        <label key={peer} className="identity-form__option">
          <input type="checkbox" checked onChange={() => toggle(peer, false)} data-testid={`identity-peer:${peer}`} />
          <span className="identity-form__option-name"><AgentName id={peer} /></span>
        </label>
      ))}
      {listed.length + unlisted.length === 0 ? <span className="identity-form__hint">No other agent to connect to yet.</span> : null}
    </div>
  );
}

/**
 * What a change came to, in words: the outcome's headline and what follows
 * from it, a refusal's sentence, or that no answer came. Ids stay in
 * Technical details. A request that waits for a decision shows when the
 * decision closes and offers its withdrawal; a change that may be sent
 * again offers it.
 */
export function IdentityResultNotice({
  change,
  result,
  name,
  busy,
  onRetry,
  onWithdraw,
  onReload,
}: {
  change: IdentityChangeKind;
  result: ShownResult;
  /** The agent as people know it. */
  name: string;
  busy?: boolean;
  /** Sends the same request again under the same id. */
  onRetry?: () => void;
  onWithdraw?: () => void;
  /** Reads the agent again after it changed under the form. */
  onReload?: () => void;
}): React.JSX.Element {
  if (result.kind === "note") {
    return (
      <Notice tone={result.alert ? "crit" : "neutral"} role={result.alert ? "alert" : "status"} testId="identity-notice" className="identity-notice">
        <span data-outcome={result.alert ? "unread" : "note"} className="identity-notice__title">{result.message}</span>
      </Notice>
    );
  }
  if (result.kind === "withdrawn") {
    return (
      <Notice tone="neutral" role="status" testId="identity-notice" className="identity-notice">
        <span data-outcome="withdrawn" className="identity-notice__title">The request is withdrawn</span>
        <span className="identity-notice__line">Nothing changed.</span>
      </Notice>
    );
  }
  if (result.kind === "failed") {
    return (
      <Notice
        tone="warn"
        role="alert"
        testId="identity-notice"
        className="identity-notice"
        action={onRetry ? <Button size="sm" disabled={busy} onClick={onRetry} data-testid="identity-retry">Try again</Button> : undefined}
      >
        <span data-outcome="failed" className="identity-notice__title">The console got no answer</span>
        <span className="identity-notice__line">
          The change may or may not have been made. Trying again sends the same request, so nothing is done twice.
        </span>
        <span className="identity-notice__line identity-notice__line--quiet">{result.message}</span>
      </Notice>
    );
  }
  if (result.kind === "refused") {
    const stale = result.refusal.kind === "stale_spec";
    return (
      <Notice
        tone="crit"
        role="alert"
        testId="identity-notice"
        className="identity-notice"
        action={
          result.refusal.retryable && onRetry
            ? <Button size="sm" disabled={busy} onClick={onRetry} data-testid="identity-retry">Try again</Button>
            : stale && onReload
              ? <Button size="sm" disabled={busy} onClick={onReload} data-testid="identity-reload">Read it again</Button>
              : undefined
        }
      >
        <span data-outcome="refused" className="identity-notice__title">{refusalSentence(result.refusal)}</span>
      </Notice>
    );
  }
  const { outcome } = result;
  const words = outcomeWords(change, outcome, name);
  const pending = outcome.outcome === "pending_decision";
  return (
    <Notice
      tone={VERDICT_TONE[words.tone]}
      role="status"
      testId="identity-notice"
      className="identity-notice"
      action={pending && onWithdraw ? <Button size="sm" disabled={busy} onClick={onWithdraw} data-testid="identity-withdraw">Withdraw request</Button> : undefined}
    >
      <span data-outcome={outcome.outcome} className="identity-notice__title">{words.title}</span>
      {words.lines.map((line, index) => <span key={index} className="identity-notice__line">{line}</span>)}
      {pending ? (
        <span className="identity-notice__line" data-testid="identity-deadline">
          The decision is open until {formatAbsoluteTime(outcome.deadlineMs)} (<Countdown to={outcome.deadlineMs} />).
        </span>
      ) : null}
      {"operationId" in outcome && outcome.operationId ? (
        <TechnicalDetails
          className="identity-notice__tech"
          items={[
            { label: "Operation", value: outcome.operationId },
            pending ? { label: "Decision", value: outcome.decisionId } : null,
          ]}
        />
      ) : null}
    </Notice>
  );
}

/** The request a form or a confirmation sends, its id, and how it went. */
function useChangeSender(api: IdentityChangesApi, change: IdentityChangeKind, name: string) {
  const slot = React.useRef<RequestIdSlot | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [result, setResult] = React.useState<ShownResult | null>(null);
  const alive = React.useRef(true);
  React.useEffect(() => () => { alive.current = false; }, []);
  const send = async (request: Record<string, unknown>): Promise<ShownResult> => {
    // The same request keeps its id; a changed request takes a new one.
    slot.current = requestIdFor(slot.current, change, request, api.mintRequestId);
    setBusy(true);
    const answer = await api.change(change, request, slot.current.requestId, name);
    if (alive.current) {
      setBusy(false);
      setResult(answer);
    }
    return answer;
  };
  const withdraw = async () => {
    if (!slot.current) return;
    setBusy(true);
    const answer = await api.withdraw(slot.current.requestId);
    if (alive.current) {
      setBusy(false);
      setResult(answer);
    }
  };
  return { busy, result, setResult, send, withdraw };
}

function fieldErrorOf(change: IdentityChangeKind, refusal: IdentityRefusal): FieldErrors | null {
  const field = refusalFormField(change, refusal.field);
  return field ? { [field]: refusalSentence(refusal) } : null;
}

/** Whether a result leaves the form open for another try. */
function keepsForm(result: ShownResult | null): boolean {
  return result === null || result.kind === "refused" || result.kind === "failed" || result.kind === "note";
}

function useDirectory(api: IdentityChangesApi) {
  const [directory, setDirectory] = React.useState<IdentityDirectory | null>(null);
  const [error, setError] = React.useState("");
  const load = React.useRef(api.loadDirectory);
  load.current = api.loadDirectory;
  React.useEffect(() => {
    let active = true;
    load.current().then(
      (value) => { if (active) setDirectory(value); },
      (failure) => { if (active) setError(errorMessage(failure)); },
    );
    return () => { active = false; };
  }, []);
  return { directory, error };
}

/**
 * New agent: a profile of the team (each shown with what the host's rules
 * give it), a name, and what the agent starts with. The form writes the
 * request as it is; the gateway decides it, and a refusal shows at the
 * field it names.
 */
export function NewAgentForm({ api, onClose }: { api: IdentityChangesApi; onClose: () => void }): React.JSX.Element {
  const [draft, setDraft] = React.useState<NewAgentDraft>(EMPTY_NEW_AGENT_DRAFT);
  const [errors, setErrors] = React.useState<FieldErrors>({});
  const { directory, error: directoryError } = useDirectory(api);
  const shownName = draft.displayName.trim() || draft.name.trim();
  const sender = useChangeSender(api, "create", shownName);
  const set = <K extends keyof NewAgentDraft>(key: K, field: IdentityFormField, value: NewAgentDraft[K]) => {
    setDraft((current) => ({ ...current, [key]: value }));
    setErrors((current) => (current[field] ? { ...current, [field]: undefined } : current));
  };
  const submit = async () => {
    setErrors({});
    const answer = await sender.send(createAgentsRequest(draft));
    if (answer.kind === "refused") {
      const fieldError = fieldErrorOf("create", answer.refusal);
      if (fieldError) {
        setErrors(fieldError);
        sender.setResult(null);
      }
    }
  };
  const complete = Boolean(draft.name.trim() && draft.profile);
  const open = keepsForm(sender.result);
  return (
    <form
      className="identity-form"
      data-testid="identity-new-agent"
      aria-label="New agent"
      onSubmit={(event) => {
        event.preventDefault();
        if (complete && !sender.busy) void submit();
      }}
    >
      {open ? (
        directoryError ? (
          <Notice tone="crit" role="alert" testId="identity-directory-error">
            The team's profiles could not be read: {directoryError}
          </Notice>
        ) : !directory ? (
          <LoadingState label="Loading the team's profiles" rows={3} />
        ) : (
          <>
            <Field field="profile" label="Profile" hint="The profile gives the agent its model, tools and skills." error={errors.profile}>
              {(ids) => <ProfilePicker directory={directory} value={draft.profile} onChange={(profile) => set("profile", "profile", profile)} {...ids} />}
            </Field>
            <Field field="name" label="Name" hint="How the agent is addressed. It cannot be changed later, and a removed agent's name is never free again." error={errors.name}>
              {(ids) => (
                <input
                  className="cc-input"
                  value={draft.name}
                  onChange={(event) => set("name", "name", event.target.value)}
                  aria-label="Name"
                  aria-describedby={ids.describedBy}
                  aria-invalid={ids.invalid || undefined}
                  autoComplete="off"
                  spellCheck={false}
                  data-testid="identity-field:name"
                />
              )}
            </Field>
            <Field field="display_name" label="Display name" hint="Optional. What people see; you can change it later." error={errors.display_name}>
              {(ids) => (
                <input
                  className="cc-input"
                  value={draft.displayName}
                  onChange={(event) => set("displayName", "display_name", event.target.value)}
                  aria-label="Display name"
                  aria-describedby={ids.describedBy}
                  aria-invalid={ids.invalid || undefined}
                  data-testid="identity-field:display_name"
                />
              )}
            </Field>
            <Field field="instructions" label="Instructions" hint="Optional. The agent's standing brief." error={errors.instructions}>
              {(ids) => (
                <textarea
                  className="cc-input"
                  rows={5}
                  value={draft.instructions}
                  onChange={(event) => set("instructions", "instructions", event.target.value)}
                  aria-label="Instructions"
                  aria-describedby={ids.describedBy}
                  aria-invalid={ids.invalid || undefined}
                  data-testid="identity-field:instructions:0"
                />
              )}
            </Field>
            <Field field="peers" label="Peers" hint="Optional. The agents it can message. Nothing else is connected to it." error={errors.peers}>
              {(ids) => <PeerPicker directory={directory} value={draft.peers} onChange={(peers) => set("peers", "peers", peers)} describedBy={ids.describedBy} />}
            </Field>
            <Field field="first_message" label="First message" hint="Optional. Its first turn, run once. Without one it starts with the first message it receives." error={errors.first_message}>
              {(ids) => (
                <textarea
                  className="cc-input"
                  rows={3}
                  value={draft.firstMessage}
                  onChange={(event) => set("firstMessage", "first_message", event.target.value)}
                  aria-label="First message"
                  aria-describedby={ids.describedBy}
                  aria-invalid={ids.invalid || undefined}
                  data-testid="identity-field:first_message"
                />
              )}
            </Field>
          </>
        )
      ) : null}
      {sender.result ? (
        <IdentityResultNotice
          change="create"
          result={sender.result}
          name={shownName}
          busy={sender.busy}
          onRetry={() => void submit()}
          onWithdraw={() => void sender.withdraw()}
        />
      ) : null}
      <div className="cc-btn-row identity-form__actions">
        {open ? (
          <Button type="submit" variant="primary" disabled={!complete || sender.busy || !directory} data-testid="identity-submit">
            {sender.busy ? "Creating" : "Create agent"}
          </Button>
        ) : null}
        <Button variant="ghost" onClick={onClose} data-testid="identity-cancel">{open ? "Cancel" : "Close"}</Button>
      </div>
    </form>
  );
}

/**
 * Edit one agent: its display name, instructions, context, labels, profile
 * and peers, against the record read when the form opened. Only what the
 * form changed is sent, at that record's spec digest.
 */
export function EditAgentForm({
  api,
  name,
  displayName,
  onClose,
}: {
  api: IdentityChangesApi;
  /** The agent's name: what the change names. */
  name: string;
  /** The agent as people know it. */
  displayName: string;
  onClose: () => void;
}): React.JSX.Element {
  const [record, setRecord] = React.useState<AgentRecord | null>(null);
  const [draft, setDraft] = React.useState<AgentEditDraft | null>(null);
  const [loadError, setLoadError] = React.useState("");
  const [errors, setErrors] = React.useState<FieldErrors>({});
  const { directory, error: directoryError } = useDirectory(api);
  const sender = useChangeSender(api, "update", displayName);
  const loadRecord = React.useRef(api.loadRecord);
  loadRecord.current = api.loadRecord;
  const read = React.useCallback(() => {
    let active = true;
    setLoadError("");
    loadRecord.current(name).then(
      (value) => {
        if (!active) return;
        setRecord(value);
        setDraft(editDraftOf(value));
      },
      (failure) => { if (active) setLoadError(refusalMessage(failure)); },
    );
    return () => { active = false; };
  }, [name]);
  React.useEffect(() => read(), [read]);

  const set = <K extends keyof AgentEditDraft>(key: K, field: IdentityFormField, value: AgentEditDraft[K]) => {
    setDraft((current) => (current ? { ...current, [key]: value } : current));
    setErrors((current) => (current[field] ? { ...current, [field]: undefined } : current));
  };
  const submit = async () => {
    if (!record || !draft) return;
    setErrors({});
    const built = updateAgentRequest(record, draft);
    if ("contextError" in built) {
      setErrors({ context: built.contextError });
      return;
    }
    if (!built.changed) {
      sender.setResult({ kind: "note", message: "Nothing is changed yet." });
      return;
    }
    const answer = await sender.send(built.request);
    if (answer.kind === "refused") {
      const fieldError = fieldErrorOf("update", answer.refusal);
      if (fieldError) {
        setErrors(fieldError);
        sender.setResult(null);
      }
    }
  };
  const reload = () => {
    sender.setResult(null);
    read();
  };
  const open = keepsForm(sender.result);
  const ready = record && draft && directory;
  const failed = loadError || directoryError;
  return (
    <form
      className="identity-form"
      data-testid={`identity-edit:${name}`}
      aria-label={`Edit ${displayName}`}
      onSubmit={(event) => {
        event.preventDefault();
        if (ready && !sender.busy) void submit();
      }}
    >
      {open ? (
        failed ? (
          <Notice tone="crit" role="alert" testId="identity-load-error">{failed}</Notice>
        ) : !ready ? (
          <LoadingState label="Loading the agent's record" rows={3} />
        ) : (
          <>
            <Field field="display_name" label="Display name" error={errors.display_name}>
              {(ids) => (
                <input
                  className="cc-input"
                  value={draft.displayName}
                  onChange={(event) => set("displayName", "display_name", event.target.value)}
                  aria-label="Display name"
                  aria-describedby={ids.describedBy}
                  aria-invalid={ids.invalid || undefined}
                  data-testid="identity-field:display_name"
                />
              )}
            </Field>
            <Field
              field="instructions"
              label="Instructions"
              hint="New instructions reach the agent at its next run. Earlier instructions are never rewritten."
              error={errors.instructions}
            >
              {(ids) => (
                <>
                  {draft.instructions.map((section, index) => (
                    <textarea
                      key={index}
                      className="cc-input"
                      rows={5}
                      value={section}
                      onChange={(event) => set("instructions", "instructions", draft.instructions.map((entry, at) => (at === index ? event.target.value : entry)))}
                      aria-label={draft.instructions.length > 1 ? `Instructions, section ${index + 1}` : "Instructions"}
                      aria-describedby={ids.describedBy}
                      aria-invalid={ids.invalid || undefined}
                      data-testid={`identity-field:instructions:${index}`}
                    />
                  ))}
                </>
              )}
            </Field>
            <Field field="profile" label="Profile" hint="A new profile moves the agent to that profile's model and tools, on the same conversation." error={errors.profile}>
              {(ids) => <ProfilePicker directory={directory} value={draft.profile} onChange={(profile) => set("profile", "profile", profile)} {...ids} />}
            </Field>
            <Field field="peers" label="Peers" hint="The agents it can message." error={errors.peers}>
              {(ids) => <PeerPicker directory={directory} value={draft.peers} exclude={name} onChange={(peers) => set("peers", "peers", peers)} describedBy={ids.describedBy} />}
            </Field>
            <Field field="labels" label="Labels" error={errors.labels}>
              {(ids) => (
                <div className="identity-form__labels" aria-describedby={ids.describedBy}>
                  {draft.labels.map((label, index) => (
                    <div className="identity-form__label-row" key={index}>
                      <input
                        className="cc-input"
                        value={label.key}
                        placeholder="Key"
                        aria-label={`Label ${index + 1} key`}
                        onChange={(event) => set("labels", "labels", draft.labels.map((entry, at) => (at === index ? { ...entry, key: event.target.value } : entry)))}
                        data-testid={`identity-label-key:${index}`}
                      />
                      <input
                        className="cc-input"
                        value={label.value}
                        placeholder="Value"
                        aria-label={`Label ${index + 1} value`}
                        onChange={(event) => set("labels", "labels", draft.labels.map((entry, at) => (at === index ? { ...entry, value: event.target.value } : entry)))}
                        data-testid={`identity-label-value:${index}`}
                      />
                      <Button size="sm" variant="ghost" onClick={() => set("labels", "labels", draft.labels.filter((_, at) => at !== index))} data-testid={`identity-label-remove:${index}`}>
                        Remove
                      </Button>
                    </div>
                  ))}
                  <Button size="sm" onClick={() => set("labels", "labels", [...draft.labels, { key: "", value: "" }])} data-testid="identity-label-add">
                    Add a label
                  </Button>
                </div>
              )}
            </Field>
            <Field field="context" label="Context" hint="Optional, as JSON. Descriptive only; it is replaced whole and gives the agent no permission." error={errors.context}>
              {(ids) => (
                <textarea
                  className="cc-input identity-form__code"
                  rows={4}
                  value={draft.context}
                  onChange={(event) => set("context", "context", event.target.value)}
                  aria-label="Context"
                  aria-describedby={ids.describedBy}
                  aria-invalid={ids.invalid || undefined}
                  spellCheck={false}
                  data-testid="identity-field:context"
                />
              )}
            </Field>
          </>
        )
      ) : null}
      {sender.result ? (
        <IdentityResultNotice
          change="update"
          result={sender.result}
          name={displayName}
          busy={sender.busy}
          onRetry={() => void submit()}
          onWithdraw={() => void sender.withdraw()}
          onReload={reload}
        />
      ) : null}
      <div className="cc-btn-row identity-form__actions">
        {open && !failed ? (
          <Button type="submit" variant="primary" disabled={!ready || sender.busy} data-testid="identity-submit">
            {sender.busy ? "Saving" : "Save changes"}
          </Button>
        ) : null}
        <Button variant="ghost" onClick={onClose} data-testid="identity-cancel">{open ? "Cancel" : "Close"}</Button>
      </div>
    </form>
  );
}

/** A failed read in words: a refusal's own sentence, else the failure. */
function refusalMessage(error: unknown): string {
  const rpc = (error as { rpcError?: { data?: { detail?: unknown } } } | null)?.rpcError;
  const detail = rpc?.data?.detail;
  return typeof detail === "string" && detail.trim() ? detail : errorMessage(error);
}

/**
 * The confirmation of a change of one agent as a whole: archive, unarchive
 * or remove. It names the agent as people know it and says what happens;
 * a removal says that the name can never be used again and that archive is
 * the reversible choice. On Confirm it reads the agent's record once for
 * its spec digest and sends the change; trying again sends that request.
 */
export function AgentChangeConfirm({
  api,
  change,
  name,
  displayName,
  onClose,
  onArchiveInstead,
}: {
  api: IdentityChangesApi;
  change: "archive" | "unarchive" | "remove";
  name: string;
  displayName: string;
  onClose: () => void;
  /** Offered beside a removal when the viewer may archive the agent. */
  onArchiveInstead?: () => void;
}): React.JSX.Element {
  const words = agentChangeConfirmWords(change, displayName);
  const sender = useChangeSender(api, change, displayName);
  const request = React.useRef<Record<string, unknown> | null>(null);
  const [reading, setReading] = React.useState(false);
  const questionId = React.useId();
  const consequenceId = React.useId();
  const confirm = async () => {
    if (!request.current) {
      setReading(true);
      try {
        request.current = agentRefRequest(await api.loadRecord(name));
      } catch (error) {
        // The record could not be read, so no change was sent: the
        // gateway's sentence when it refused the read.
        sender.setResult({ kind: "note", message: refusalMessage(error), alert: true });
        return;
      } finally {
        setReading(false);
      }
    }
    await sender.send(request.current);
  };
  const reload = () => {
    // The agent changed since it was read: a new read is a new request.
    request.current = null;
    sender.setResult(null);
  };
  const busy = reading || sender.busy;
  const open = keepsForm(sender.result);
  return (
    <div
      role="alertdialog"
      aria-labelledby={questionId}
      aria-describedby={consequenceId}
      className="lifecycle-confirm identity-confirm"
      data-action={change}
      data-testid={`identity-confirm:${name}:${change}`}
    >
      {open ? (
        <>
          <p className="lifecycle-confirm__question" id={questionId}>
            <StatusMark tone={change === "remove" ? "crit" : "warn"} />
            {words.question}
          </p>
          <p className="lifecycle-confirm__consequence" id={consequenceId}>{words.consequence}</p>
        </>
      ) : null}
      {sender.result ? (
        <IdentityResultNotice
          change={change}
          result={sender.result}
          name={displayName}
          busy={busy}
          onRetry={() => void confirm()}
          onWithdraw={() => void sender.withdraw()}
          onReload={reload}
        />
      ) : null}
      <div className="cc-btn-row lifecycle-confirm__actions">
        {open ? (
          <Button variant={change === "remove" ? "danger" : "primary"} size="sm" disabled={busy} onClick={() => void confirm()} data-testid="identity-confirm-yes">
            {words.confirm}
          </Button>
        ) : null}
        {open && change === "remove" && onArchiveInstead ? (
          <Button size="sm" disabled={busy} onClick={onArchiveInstead} data-testid="identity-confirm-archive-instead">Archive instead</Button>
        ) : null}
        <Button size="sm" variant={open ? "default" : "ghost"} onClick={onClose} data-testid="identity-confirm-cancel">{open ? "Cancel" : "Close"}</Button>
      </div>
    </div>
  );
}

/** The change of an agent a panel has open. */
export type OpenAgentChange = "edit" | "archive" | "unarchive" | "remove";

/** The form or confirmation of the change a panel has open. */
export function AgentChangePanel({
  api,
  change,
  name,
  displayName,
  controls,
  onClose,
  onSwitch,
}: {
  api: IdentityChangesApi;
  change: OpenAgentChange;
  name: string;
  displayName: string;
  controls: Pick<AgentIdentityControls, "archive">;
  onClose: () => void;
  onSwitch: (change: OpenAgentChange) => void;
}): React.JSX.Element {
  if (change === "edit") return <EditAgentForm api={api} name={name} displayName={displayName} onClose={onClose} />;
  return (
    <AgentChangeConfirm
      // A switch from Remove to Archive is another action: a new request.
      key={change}
      api={api}
      change={change}
      name={name}
      displayName={displayName}
      onClose={onClose}
      onArchiveInstead={change === "remove" && controls.archive ? () => onSwitch("archive") : undefined}
    />
  );
}

/** The buttons that open a change of one agent: only those the gateway
 * offers this viewer for this agent. */
export function AgentChangeButtons({
  controls,
  open,
  onOpen,
  testIdPrefix,
}: {
  controls: AgentIdentityControls;
  open: OpenAgentChange | null;
  onOpen: (change: OpenAgentChange) => void;
  testIdPrefix: string;
}): React.JSX.Element | null {
  if (!controls.edit && !controls.archive && !controls.remove) return null;
  return (
    <div className="cc-btn-row" role="group" aria-label="Agent record">
      {controls.edit ? (
        <Button size="sm" aria-expanded={open === "edit"} onClick={() => onOpen("edit")} data-testid={`${testIdPrefix}:edit`}>Edit</Button>
      ) : null}
      {controls.archive ? (
        <Button size="sm" aria-expanded={open === "archive"} onClick={() => onOpen("archive")} data-testid={`${testIdPrefix}:archive`}>Archive</Button>
      ) : null}
      {controls.remove ? (
        <Button size="sm" variant="danger" aria-expanded={open === "remove"} onClick={() => onOpen("remove")} data-testid={`${testIdPrefix}:remove`}>Remove</Button>
      ) : null}
    </div>
  );
}

/**
 * The archived agents: each by its name as people know it, with Unarchive
 * and Remove where the gateway offers them. An archived agent has no
 * member, so the roster's own rows do not hold it.
 */
export function ArchivedAgents({
  api,
  roster,
  onOpen,
}: {
  api: IdentityChangesApi;
  /** The roster as last read: the archived agents are read again with it,
   * so a change somebody else made shows without a change of this page. */
  roster?: unknown;
  /** Opens the change of an archived agent. */
  onOpen: (agent: AgentSummary, change: "unarchive" | "remove") => void;
}): React.JSX.Element | null {
  const [agents, setAgents] = React.useState<AgentSummary[]>([]);
  const load = React.useRef(api.loadArchived);
  load.current = api.loadArchived;
  React.useEffect(() => {
    let active = true;
    // A list that cannot be read shows nothing: the roster stays usable.
    load.current().then((value) => { if (active) setAgents(value); }, () => {});
    return () => { active = false; };
  }, [api.revision, roster]);
  const controls = archivedAgentControls(api.flags);
  if (agents.length === 0) return null;
  return (
    <section className="identity-archived" aria-label="Archived agents" data-testid="roster-archived">
      <div className="roster__group-head">Archived <span className="cc-count">{agents.length}</span></div>
      <ul className="identity-archived__list">
        {agents.map((agent) => (
          <li key={agent.name} className="identity-archived__row" data-testid={`roster-archived:${agent.name}`}>
            <span className="identity-archived__name" title={agent.name}>{agentShownName(agent)}</span>
            <span className="identity-archived__meta">{humanizeEnum(agent.profile)}</span>
            <span className="cc-btn-row identity-archived__actions">
              {controls.unarchive ? (
                <Button size="sm" onClick={() => onOpen(agent, "unarchive")} data-testid={`roster-archived-action:${agent.name}:unarchive`}>
                  Unarchive
                </Button>
              ) : null}
              {controls.remove ? (
                <Button size="sm" variant="danger" onClick={() => onOpen(agent, "remove")} data-testid={`roster-archived-action:${agent.name}:remove`}>
                  Remove
                </Button>
              ) : null}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

const CHANGE_WORDS: Record<IdentityChangeKind, string> = {
  create: "New agent",
  update: "Edit",
  archive: "Archive",
  unarchive: "Unarchive",
  remove: "Remove",
};

/** This page's requests that wait for a person's decision, each with when
 * the decision closes and its withdrawal. */
export function PendingIdentityRequests({ api }: { api: IdentityChangesApi }): React.JSX.Element | null {
  const [busy, setBusy] = React.useState("");
  const [error, setError] = React.useState("");
  if (api.pending.length === 0 && !error) return null;
  const withdraw = async (requestId: string) => {
    setBusy(requestId);
    setError("");
    const answer = await api.withdraw(requestId);
    setBusy("");
    if (answer.kind === "refused") setError(refusalSentence(answer.refusal));
    else if (answer.kind === "failed") setError(answer.message);
  };
  return (
    <section className="identity-pending" aria-label="Requests waiting for a decision" data-testid="identity-pending">
      {api.pending.map((entry) => (
        <Notice
          key={entry.requestId}
          tone="warn"
          role="status"
          className="identity-notice"
          testId={`identity-pending:${entry.name}`}
          action={<Button size="sm" disabled={busy === entry.requestId} onClick={() => void withdraw(entry.requestId)} data-testid={`identity-pending-withdraw:${entry.name}`}>Withdraw request</Button>}
        >
          <span className="identity-notice__title">{CHANGE_WORDS[entry.change]}: {entry.name} waits for a person's decision</span>
          <span className="identity-notice__line">
            Nothing has changed yet. The decision is open until {formatAbsoluteTime(entry.deadlineMs)} (<Countdown to={entry.deadlineMs} />).
          </span>
        </Notice>
      ))}
      {error ? <Notice tone="crit" role="alert" testId="identity-pending-error">{error}</Notice> : null}
    </section>
  );
}

/**
 * Who asked for an agent and when, as its record says: a record, not a
 * mark on the agent. Shown where the record carries it; the spec digest
 * and revision stay in Technical details.
 */
export function AgentRecordFacts({
  load,
  name,
  refreshToken,
}: {
  /** Reads the agent's record. */
  load: (name: string) => Promise<AgentRecord>;
  name: string;
  /** Reads the record again when it changes. */
  refreshToken?: unknown;
}): React.JSX.Element | null {
  const [record, setRecord] = React.useState<AgentRecord | null>(null);
  const loadRef = React.useRef(load);
  loadRef.current = load;
  React.useEffect(() => {
    let active = true;
    // An agent without a readable record shows none.
    loadRef.current(name).then((value) => { if (active) setRecord(value); }, () => { if (active) setRecord(null); });
    return () => { active = false; };
  }, [name, refreshToken]);
  if (!record) return null;
  const provenance = record.provenance;
  const requester = provenance ? requesterWords(provenance, principalName) : null;
  return (
    <section className="cc-section identity-record" data-testid={`inspect-record:${name}`} aria-label="Record">
      <div className="cc-section-head"><h3 className="cc-section-head__label">Record</h3></div>
      <div className="cc-section__content cc-section__content--padded">
        <KeyValueList
          items={[
            requester
              ? {
                  label: "Asked for by",
                  value: requester.kind === "agent"
                    ? <AgentName id={requester.identity} link />
                    : requester.kind === "person"
                      ? <span title={requester.key}>{requester.name}</span>
                      : requester.words,
                }
              : null,
            provenance ? { label: "When", value: <time dateTime={new Date(provenance.atMs).toISOString()}>{formatAbsoluteTime(provenance.atMs)}</time> } : null,
            provenance?.consentForm ? { label: "Consent", value: consentWords(provenance.consentForm) } : null,
            record.openOperations.length
              ? { label: "Open requests", value: `${record.openOperations.length} waiting` }
              : null,
          ]}
        />
        <TechnicalDetails
          items={[
            { label: "Name", value: record.name },
            { label: "Spec digest", value: record.specDigest },
            typeof record.specRevision === "number" ? { label: "Spec revision", value: record.specRevision } : null,
          ]}
        />
      </div>
    </section>
  );
}
