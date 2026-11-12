import React from "react";

import {
  EMPTY_NEW_AGENT_DRAFT,
  REACH_CHOICES,
  RUN_MODE_CHOICES,
  agentChangeConfirmWords,
  agentRefRequest,
  agentShownName,
  archivedAgentControls,
  consentWords,
  createAgentsRequest,
  editDraftOf,
  keepFirstSchedules,
  outcomeWords,
  planWords,
  plannedSchedulesWords,
  answeredRefusalOf,
  readRefusalSentence,
  refusalDetail,
  refusalFormField,
  refusalSentence,
  requestIdFor,
  requesterWords,
  runModeWords,
  schedulesOweRetry,
  updateAgentRequest,
  verdictWords,
  waitsForItsHost,
  type AgentEditDraft,
  type AgentIdentityControls,
  type AgentRecord,
  type AgentSummary,
  type DirectoryVerdict,
  type IdentityChangeKind,
  type IdentityFormField,
  type IdentityOutcome,
  type IdentityRefusal,
  type NewAgentDraft,
  type RequestIdSlot,
  type SettingChoice,
} from "../lib/identity-changes";
import type { IdentityChangeResult, IdentityChangesApi, IdentityDirectory, IdentityPlanResult } from "../lib/use-identity-changes";
import { errorMessage } from "../lib/errors";
import { principalName } from "./decision-cards";
import { useConfirmDialog } from "./LifecycleConfirm";
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

/** What the console says of a refusal or a failure: its own sentence, and
 * for the Details disclosure the gateway's own, where it gave one. */
interface Said {
  sentence: string;
  detail?: string;
}

function saidOf(refusal: IdentityRefusal): Said {
  const detail = refusalDetail(refusal);
  return { sentence: refusalSentence(refusal), ...(detail ? { detail } : {}) };
}

/** A failed read in words: a refusal in the console's sentence for a read
 * that was refused, and anything else as an answer that did not come. */
function readFailure(error: unknown): Said {
  const refusal = answeredRefusalOf(error);
  if (!refusal) return { sentence: "The console got no answer it can read.", detail: errorMessage(error) };
  const detail = refusalDetail(refusal);
  return { sentence: readRefusalSentence(refusal), ...(detail ? { detail } : {}) };
}

/**
 * The gateway's own sentence of a refusal, behind a disclosure: the
 * console's sentence is what a person reads, and this is there for whoever
 * needs the exact answer.
 */
export function RefusalDetails({ detail }: { detail?: string | null }): React.JSX.Element | null {
  if (!detail) return null;
  return (
    <TechnicalDetails summary="Details" inline={false} copyAll={false} showCount={false} className="identity-refusal__details" testId="identity-refusal-details">
      <span className="identity-refusal__gateway">{detail}</span>
    </TechnicalDetails>
  );
}

type FieldErrors = Partial<Record<IdentityFormField, Said>>;

/** What the notice under a form or a confirmation shows. A note is the
 * console's own word: that nothing is changed yet, or (an alert) that the
 * agent's record could not be read. `settled` is a request that waited
 * for a decision and no longer does. */
type ShownResult =
  | IdentityChangeResult
  | { kind: "withdrawn" }
  | { kind: "settled" }
  | { kind: "note"; message: string; detail?: string; alert?: boolean };

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
  error?: Said;
  children: (ids: { describedBy?: string; invalid: boolean }) => React.ReactNode;
}) {
  const errorId = React.useId();
  return (
    <div className="identity-form__field" data-field={field} data-invalid={error ? "true" : undefined}>
      <span className="identity-form__label">{label}</span>
      {children({ describedBy: error ? errorId : undefined, invalid: Boolean(error) })}
      {hint ? <span className="identity-form__hint">{hint}</span> : null}
      {error ? (
        <>
          <span className="identity-form__error" id={errorId} role="alert" data-testid={`identity-field-error:${field}`}>
            {error.sentence}
          </span>
          <RefusalDetails detail={error.detail} />
        </>
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

/** The profiles of the directory, one to pick. With `verdicts` each shows
 * what the host's rules give a new agent of it, and one they do not allow
 * cannot be picked. Without, every profile can be picked and none carries a
 * verdict: the directory's verdicts are a create's, not those of another
 * change. */
function ProfilePicker({
  directory,
  value,
  onChange,
  describedBy,
  invalid,
  verdicts = true,
}: {
  directory: IdentityDirectory;
  value: string;
  onChange: (profile: string) => void;
  describedBy?: string;
  invalid: boolean;
  verdicts?: boolean;
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
        const pickable = !verdicts || verdictWords(entry).pickable;
        return (
          <label key={entry.profile} className={cx("identity-form__option", !pickable && "is-refused")}>
            <input
              type="radio"
              name={name}
              value={entry.profile}
              checked={value === entry.profile}
              disabled={!pickable}
              onChange={() => onChange(entry.profile)}
              data-testid={`identity-profile:${entry.profile}`}
            />
            <span className="identity-form__option-name">{humanizeEnum(entry.profile)}</span>
            {verdicts && entry.verdict ? <Verdict entry={entry} /> : null}
          </label>
        );
      })}
      {listed.length === 0 ? <span className="identity-form__hint">This team lists no profile.</span> : null}
    </div>
  );
}

/**
 * One setting of an agent that has a few choices (who reaches it, how it
 * runs): each choice in words with what it means, one to pick. The wire's
 * value is the radio's value and never text.
 */
function SettingPicker({
  label,
  choices,
  value,
  onChange,
  testIdPrefix,
  describedBy,
  invalid,
}: {
  label: string;
  choices: readonly SettingChoice[];
  value: string;
  onChange: (value: string) => void;
  /** A choice's test id is this and its value; the choice without a value
   * (as the profile says) reads `profile`. */
  testIdPrefix: string;
  describedBy?: string;
  invalid: boolean;
}) {
  const name = React.useId();
  return (
    <div className="identity-form__options" role="radiogroup" aria-label={label} aria-describedby={describedBy} aria-invalid={invalid || undefined}>
      {choices.map((choice) => (
        <label key={choice.value} className="identity-form__option">
          <input
            type="radio"
            name={name}
            value={choice.value}
            checked={value === choice.value}
            onChange={() => onChange(choice.value)}
            data-testid={`${testIdPrefix}:${choice.value || "profile"}`}
          />
          <span className="identity-form__option-name">{choice.label}</span>
          <span className="identity-form__option-hint">{choice.hint}</span>
        </label>
      ))}
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
        <RefusalDetails detail={result.detail} />
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
  if (result.kind === "settled") {
    return (
      <Notice tone="neutral" role="status" testId="identity-notice" className="identity-notice">
        <span data-outcome="settled" className="identity-notice__title">The request is no longer waiting for a decision</span>
        <span className="identity-notice__line">It was decided or withdrawn. The roster shows the team as it is now.</span>
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
        <RefusalDetails detail={refusalDetail(result.refusal)} />
      </Notice>
    );
  }
  const { outcome } = result;
  const words = outcomeWords(change, outcome, name);
  const pending = outcome.outcome === "pending_decision";
  // A change that could not follow every schedule of its agent: the same
  // request sent again follows the rest.
  const followAgain = schedulesOweRetry(outcome) && onRetry;
  const scheduleIds = "agents" in outcome
    ? outcome.agents.flatMap((agent) => agent.schedules?.schedules ?? []).map((schedule) => schedule.scheduleId)
    : [];
  return (
    <Notice
      tone={VERDICT_TONE[words.tone]}
      role="status"
      testId="identity-notice"
      className="identity-notice"
      action={
        pending && onWithdraw
          ? <Button size="sm" disabled={busy} onClick={onWithdraw} data-testid="identity-withdraw">Withdraw request</Button>
          : followAgain
            ? <Button size="sm" disabled={busy} onClick={onRetry} data-testid="identity-retry">Try again</Button>
            : undefined
      }
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
            ...scheduleIds.map((scheduleId) => ({ label: "Schedule", value: scheduleId })),
          ]}
        />
      ) : null}
    </Notice>
  );
}

/** The request a form or a confirmation sends, its id, and how it went. */
function useChangeSender(api: IdentityChangesApi, change: IdentityChangeKind, name: string) {
  const slot = React.useRef<RequestIdSlot | null>(null);
  // The first outcome this request was answered with: the same request
  // answered again is never taken whole in its place.
  const first = React.useRef<{ requestId: string; outcome: IdentityOutcome } | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [result, setResult] = React.useState<ShownResult | null>(null);
  const alive = React.useRef(true);
  React.useEffect(() => () => { alive.current = false; }, []);
  const send = async (request: Record<string, unknown>): Promise<ShownResult> => {
    // The same request keeps its id; a changed request takes a new one.
    slot.current = requestIdFor(slot.current, change, request, api.mintRequestId);
    const requestId = slot.current.requestId;
    setBusy(true);
    let answer = await api.change(change, request, requestId, name);
    if (answer.kind === "outcome") {
      if (first.current?.requestId === requestId) {
        // What the first answer said of the agent's schedules stays: a
        // request answered again states nothing of them.
        answer = { kind: "outcome", outcome: keepFirstSchedules(first.current.outcome, answer.outcome) };
      } else {
        first.current = { requestId, outcome: answer.outcome };
      }
    }
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
  // A request that waits for a decision is one of the page's pending
  // requests. Once it has left them (decided, or withdrawn from another
  // place of the console) it waits no longer, and the notice stops saying
  // that it does.
  const waiting = result?.kind === "outcome" && result.outcome.outcome === "pending_decision";
  const requestId = slot.current?.requestId;
  const listed = api.pending.some((entry) => entry.requestId === requestId);
  const wasListed = React.useRef(false);
  React.useEffect(() => {
    if (!waiting) {
      wasListed.current = false;
    } else if (listed) {
      wasListed.current = true;
    } else if (wasListed.current) {
      wasListed.current = false;
      setResult({ kind: "settled" });
    }
  }, [waiting, listed]);
  return { busy, result, setResult, send, withdraw };
}

function fieldErrorOf(change: IdentityChangeKind, refusal: IdentityRefusal): FieldErrors | null {
  const field = refusalFormField(change, refusal.field);
  return field ? { [field]: saidOf(refusal) } : null;
}

/** Whether a result leaves the form open for another try. */
function keepsForm(result: ShownResult | null): boolean {
  return result === null || result.kind === "refused" || result.kind === "failed" || result.kind === "note";
}

function useDirectory(api: IdentityChangesApi) {
  const [directory, setDirectory] = React.useState<IdentityDirectory | null>(null);
  const [error, setError] = React.useState<Said | null>(null);
  const load = React.useRef(api.loadDirectory);
  load.current = api.loadDirectory;
  React.useEffect(() => {
    let active = true;
    load.current().then(
      (value) => { if (active) setDirectory(value); },
      (failure) => { if (active) setError(readFailure(failure)); },
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
            <span className="identity-notice__title">The team's profiles could not be read.</span>
            <span className="identity-notice__line">{directoryError.sentence}</span>
            <RefusalDetails detail={directoryError.detail} />
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
            <Field field="addressability" label="Reachability" error={errors.addressability}>
              {(ids) => (
                <SettingPicker
                  label="Reachability"
                  choices={REACH_CHOICES}
                  value={draft.addressability}
                  onChange={(value) => set("addressability", "addressability", value)}
                  testIdPrefix="identity-addressability"
                  {...ids}
                />
              )}
            </Field>
            <Field field="runtime_mode" label="How it runs" error={errors.runtime_mode}>
              {(ids) => (
                <SettingPicker
                  label="How it runs"
                  choices={RUN_MODE_CHOICES}
                  value={draft.runtimeMode}
                  onChange={(value) => set("runtimeMode", "runtime_mode", value)}
                  testIdPrefix="identity-runtime-mode"
                  {...ids}
                />
              )}
            </Field>
            <Field field="first_message" label="First message" hint="Optional. The agent is given it once, when it starts; what an agent does at its start follows from how it runs, which its profile decides unless you choose above." error={errors.first_message}>
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
 * What the gateway says a change of the agent's profile to `profile` would
 * get. Asked whenever the form names another profile than the record's, of
 * this agent at the spec the form was opened on; null while the profile is
 * the record's own and until the answer is in.
 */
function useProfilePlan(api: IdentityChangesApi, record: AgentRecord | null, profile: string): IdentityPlanResult | null {
  const [answer, setAnswer] = React.useState<{ key: string; result: IdentityPlanResult } | null>(null);
  const plan = React.useRef(api.plan);
  plan.current = api.plan;
  const name = record?.name ?? "";
  const digest = record?.specDigest ?? "";
  const key = record && profile && profile !== record.profile ? `${name}\n${digest}\n${profile}` : "";
  React.useEffect(() => {
    if (!key) return undefined;
    let active = true;
    void plan.current("update", { name, expected_spec_digest: digest, set: { profile } }).then((result) => {
      if (active) setAnswer({ key, result });
    });
    return () => { active = false; };
  }, [key, name, digest, profile]);
  return answer && answer.key === key ? answer.result : null;
}

/**
 * Edit one agent: its display name, instructions, context, labels, profile,
 * peers, who reaches it and how it runs, against the record read when the
 * form opened. Only what the form changed is sent, at that record's spec
 * digest.
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
  const [loadError, setLoadError] = React.useState<Said | null>(null);
  const [errors, setErrors] = React.useState<FieldErrors>({});
  const { directory, error: directoryError } = useDirectory(api);
  const sender = useChangeSender(api, "update", displayName);
  const loadRecord = React.useRef(api.loadRecord);
  loadRecord.current = api.loadRecord;
  const read = React.useCallback(() => {
    let active = true;
    setLoadError(null);
    loadRecord.current(name).then(
      (value) => {
        if (!active) return;
        setRecord(value);
        setDraft(editDraftOf(value));
      },
      (failure) => { if (active) setLoadError(readFailure(failure)); },
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
      setErrors({ context: { sentence: built.contextError } });
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
  const failed = loadError ?? directoryError;
  // A profile change is its own change under its own rules: the gateway
  // says what it would get, and a refusal of it shows at the profile.
  const profilePlan = useProfilePlan(api, record, draft?.profile ?? "");
  const planned = profilePlan?.kind === "plan" ? planWords(profilePlan.plan) : null;
  const profileError = errors.profile ?? (profilePlan?.kind === "refused" ? saidOf(profilePlan.refusal) : undefined);
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
          <Notice tone="crit" role="alert" testId="identity-load-error">
            <span className="identity-notice__title">{failed.sentence}</span>
            <RefusalDetails detail={failed.detail} />
          </Notice>
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
              hint="The result says when new instructions reach the agent. Earlier instructions are never rewritten."
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
            <Field field="profile" label="Profile" hint="A new profile moves the agent to that profile's model and tools, on the same conversation." error={profileError}>
              {(ids) => (
                <>
                  <ProfilePicker directory={directory} value={draft.profile} onChange={(profile) => set("profile", "profile", profile)} verdicts={false} {...ids} />
                  {planned && profilePlan?.kind === "plan" ? (
                    <span className="identity-form__plan" role="status" data-testid="identity-profile-plan" data-plan={profilePlan.plan.outcome}>
                      <StatusPill tone={VERDICT_TONE[planned.tone]} variant="quiet">{planned.label}</StatusPill>
                      <span>{planned.line}</span>
                    </span>
                  ) : null}
                </>
              )}
            </Field>
            <Field field="peers" label="Peers" hint="The agents it can message." error={errors.peers}>
              {(ids) => <PeerPicker directory={directory} value={draft.peers} exclude={name} onChange={(peers) => set("peers", "peers", peers)} describedBy={ids.describedBy} />}
            </Field>
            <Field field="addressability" label="Reachability" error={errors.addressability}>
              {(ids) => (
                <SettingPicker
                  label="Reachability"
                  choices={REACH_CHOICES}
                  value={draft.addressability}
                  onChange={(value) => set("addressability", "addressability", value)}
                  testIdPrefix="identity-addressability"
                  {...ids}
                />
              )}
            </Field>
            <Field
              field="runtime_mode"
              label="How it runs"
              // An update cannot hand the way an agent runs back to its
              // profile, so that choice is listed only for an agent whose
              // record names no way.
              hint={record.runtimeMode ? "It can be changed, not handed back to the profile." : undefined}
              error={errors.runtime_mode}
            >
              {(ids) => (
                <SettingPicker
                  label="How it runs"
                  choices={record.runtimeMode ? RUN_MODE_CHOICES.filter((choice) => choice.value !== "") : RUN_MODE_CHOICES}
                  value={draft.runtimeMode}
                  onChange={(value) => set("runtimeMode", "runtime_mode", value)}
                  testIdPrefix="identity-runtime-mode"
                  {...ids}
                />
              )}
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

/**
 * The confirmation of a change of one agent as a whole: archive, unarchive
 * or remove. It names the agent as people know it and says what happens;
 * a removal says that the name can never be used again and, for an agent
 * that is not archived, that archive is the reversible choice.
 *
 * When it opens it reads the agent's record once, for its spec digest, and
 * asks the gateway what the change would take with it (`meta/identity/plan`,
 * which changes nothing): where the plan lists schedules, the question says
 * what will become of them, by name. A read or a plan that fails is not
 * said, and the question then says nothing of schedules. Confirm sends the
 * change at the digest read when the dialog opened (it reads the record
 * itself where that read failed); trying again sends that request.
 */
export function AgentChangeConfirm({
  api,
  change,
  name,
  displayName,
  waiting,
  archived,
  onClose,
  onArchiveInstead,
}: {
  api: IdentityChangesApi;
  change: "archive" | "unarchive" | "remove";
  name: string;
  displayName: string;
  /** The agent still waits for its host: it never started. */
  waiting?: boolean;
  /** The agent is archived already: it left the team at its archive. */
  archived?: boolean;
  onClose: () => void;
  /** Offered beside a removal when the viewer may archive the agent. */
  onArchiveInstead?: () => void;
}): React.JSX.Element {
  const words = agentChangeConfirmWords(change, displayName, { waiting, archived });
  const sender = useChangeSender(api, change, displayName);
  const request = React.useRef<Record<string, unknown> | null>(null);
  const [reading, setReading] = React.useState(false);
  // What the plan says the change will do to the agent's schedules, in words.
  const [schedules, setSchedules] = React.useState<string | null>(null);
  // Counts up at Read it again: the record and the plan are read again.
  const [asked, setAsked] = React.useState(0);
  // The read of the record the dialog makes when it opens, while Confirm
  // has not taken it: the request it read, or null where it read none.
  const opening = React.useRef<Promise<Record<string, unknown> | null> | null>(null);
  const loadRecord = React.useRef(api.loadRecord);
  loadRecord.current = api.loadRecord;
  const plan = React.useRef(api.plan);
  plan.current = api.plan;
  const questionId = React.useId();
  const consequenceId = React.useId();
  React.useEffect(() => {
    setSchedules(null);
    opening.current = null;
    // An agent that waits for its host never started and no schedule
    // targets it: nothing is read for it before Confirm.
    if (waiting) return undefined;
    let active = true;
    const read = loadRecord.current(name).then(
      (record) => {
        if (!active) return null;
        request.current = agentRefRequest(record);
        return request.current;
      },
      // A read that fails here is not said: Confirm reads the record itself,
      // and says so when it cannot.
      () => null,
    );
    opening.current = read;
    void read.then(async (asks) => {
      if (!asks) return;
      // A plan that is not answered, is refused or says nothing of schedules
      // leaves the question without a word of them: the gateway judges the
      // change when it is sent.
      let answer: IdentityPlanResult;
      try {
        answer = await plan.current(change, asks);
      } catch {
        return;
      }
      if (active && answer.kind === "plan") setSchedules(plannedSchedulesWords(answer.plan.schedules));
    });
    return () => { active = false; };
  }, [name, change, waiting, asked]);
  const confirm = async () => {
    if (!request.current) {
      setReading(true);
      try {
        // The read made when the dialog opened may not have ended: Confirm
        // waits for it, so that the record is not read twice. Where it read
        // nothing, or none was made, the record is read now.
        const opened = opening.current;
        opening.current = null;
        request.current = (opened ? await opened : null) ?? agentRefRequest(await api.loadRecord(name));
      } catch (error) {
        // The record could not be read, so no change was sent.
        const said = readFailure(error);
        sender.setResult({ kind: "note", message: said.sentence, detail: said.detail, alert: true });
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
    // The record is read again now, and the plan asked again: the question
    // says what the change will do to the agent as it is.
    setAsked((count) => count + 1);
  };
  const busy = reading || sender.busy;
  const open = keepsForm(sender.result);
  // The dialog of a lifecycle confirmation: focus on the safe choice, Tab
  // kept inside, Escape and a click outside leave it (not while the change
  // is being sent: its answer must be seen), focus back to what opened it.
  const { rootRef, cancelRef, onKeyDown } = useConfirmDialog(onClose, busy);
  // When the question gives way to the outcome its Confirm button is gone:
  // focus moves to the one control left.
  React.useEffect(() => {
    if (!open) cancelRef.current?.focus({ preventScroll: true });
  }, [open, cancelRef]);
  return (
    <div
      ref={rootRef}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby={open ? questionId : undefined}
      aria-describedby={open ? consequenceId : undefined}
      aria-label={open ? undefined : `${CHANGE_WORDS[change]} ${displayName}`}
      className="lifecycle-confirm identity-confirm"
      data-action={change}
      data-testid={`identity-confirm:${name}:${change}`}
      onKeyDown={onKeyDown}
    >
      {open ? (
        <>
          <p className="lifecycle-confirm__question" id={questionId}>
            <StatusMark tone={change === "remove" ? "crit" : "warn"} />
            {words.question}
          </p>
          <p className="lifecycle-confirm__consequence" id={consequenceId}>
            {words.consequence}
            {schedules ? (
              // Part of what the dialog is described by, on a line of its own.
              <>{" "}<span className="identity-confirm__schedules" data-testid="identity-confirm-schedules">{schedules}</span></>
            ) : null}
          </p>
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
        <Button ref={cancelRef} size="sm" variant={open ? "default" : "ghost"} onClick={onClose} data-testid="identity-confirm-cancel">{open ? "Cancel" : "Close"}</Button>
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
  waiting,
  archived,
  onClose,
  onSwitch,
}: {
  api: IdentityChangesApi;
  change: OpenAgentChange;
  name: string;
  displayName: string;
  controls: Pick<AgentIdentityControls, "archive">;
  /** The agent still waits for its host: it never started. */
  waiting?: boolean;
  /** The agent is archived already: it left the team at its archive. */
  archived?: boolean;
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
      waiting={waiting}
      archived={archived}
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
 * The agents without a member, which the roster's own rows do not hold:
 * the archived ones, each by its name as people know it, with Unarchive and
 * Remove where the gateway offers them; and those that still wait for
 * their host's preparation, by name, with their state in words and no
 * control (nothing can be done with one yet).
 *
 * They are read when the person opens them: the roster shows the control
 * alone, without a count, and reads nothing for it. An open list is read
 * again after a change this page made, and when it is closed and opened
 * again; the roster's own reads do not read it.
 */
export function ArchivedAgents({
  api,
  onOpen,
}: {
  api: IdentityChangesApi;
  /** Opens the change of an archived agent. */
  onOpen: (agent: AgentSummary, change: "unarchive" | "remove") => void;
}): React.JSX.Element {
  const [open, setOpen] = React.useState(false);
  const [agents, setAgents] = React.useState<AgentSummary[] | null>(null);
  const [error, setError] = React.useState<Said | null>(null);
  const load = React.useRef(api.loadOffRoster);
  load.current = api.loadOffRoster;
  const listId = React.useId();
  React.useEffect(() => {
    if (!open) return undefined;
    let active = true;
    load.current().then(
      (value) => {
        if (!active) return;
        setAgents(value);
        setError(null);
      },
      (failure) => { if (active) setError(readFailure(failure)); },
    );
    return () => { active = false; };
  }, [open, api.revision]);
  const controls = archivedAgentControls(api.flags);
  // An agent that waits can be given up where the launch offers removals:
  // the gateway judges the removal when it is sent, as for an archived one.
  const canGiveUp = controls.remove;
  const toggle = () => {
    // A closed list keeps nothing: opening it reads it.
    if (open) {
      setAgents(null);
      setError(null);
    }
    setOpen(!open);
  };
  const waiting = (agents ?? []).filter(waitsForItsHost);
  const archived = (agents ?? []).filter((agent) => agent.archived);
  return (
    <div className="identity-archived">
      <Button size="sm" variant="ghost" aria-expanded={open} aria-controls={open ? listId : undefined} onClick={toggle} data-testid="roster-archived-open">
        Archived and waiting agents
      </Button>
      {open ? (
        <section className="identity-archived__section" id={listId} aria-label="Archived and waiting agents" data-testid="roster-archived">
          {error ? (
            <Notice tone="crit" role="alert" testId="roster-archived-error">
              <span className="identity-notice__title">The archived and waiting agents could not be read.</span>
              <span className="identity-notice__line">{error.sentence}</span>
              <RefusalDetails detail={error.detail} />
            </Notice>
          ) : agents === null ? (
            <LoadingState label="Loading the archived and waiting agents" rows={2} />
          ) : waiting.length + archived.length === 0 ? (
            <span className="identity-form__hint">No archived agents, and none that waits for its host.</span>
          ) : (
            <>
              {waiting.length ? (
                <>
                  <div className="roster__group-head">Waiting for their host <span className="cc-count">{waiting.length}</span></div>
                  <ul className="identity-archived__list">
                    {waiting.map((agent) => (
                      // It has no member yet, so nothing can be done with
                      // it but to give it up: where the launch offers
                      // removals, Remove is its one control. It joins the
                      // roster when its host is done.
                      <li key={agent.name} className="identity-archived__row" data-testid={`roster-waiting:${agent.name}`}>
                        <span className="identity-archived__name" title={agent.name}>{agentShownName(agent)}</span>
                        <span className="identity-archived__meta">{humanizeEnum(agent.profile)}</span>
                        <span className="cc-btn-row identity-archived__actions">
                          <StatusPill tone="warn" variant="quiet">Waiting for its host</StatusPill>
                          {canGiveUp ? (
                            <Button size="sm" variant="danger" onClick={() => onOpen(agent, "remove")} data-testid={`roster-waiting-action:${agent.name}:remove`}>
                              Remove
                            </Button>
                          ) : null}
                        </span>
                      </li>
                    ))}
                  </ul>
                  <span className="identity-form__hint identity-archived__hint">
                    Its host is still preparing it. It joins the roster when that ends.
                  </span>
                </>
              ) : null}
              {archived.length ? (
                <>
                  <div className="roster__group-head">Archived <span className="cc-count">{archived.length}</span></div>
                  <ul className="identity-archived__list">
                    {archived.map((agent) => (
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
                </>
              ) : null}
            </>
          )}
        </section>
      ) : null}
    </div>
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
  const [error, setError] = React.useState<Said | null>(null);
  if (api.pending.length === 0 && !error) return null;
  const withdraw = async (requestId: string) => {
    setBusy(requestId);
    setError(null);
    const answer = await api.withdraw(requestId);
    setBusy("");
    if (answer.kind === "refused") setError(saidOf(answer.refusal));
    else if (answer.kind === "failed") setError({ sentence: "The console got no answer. The request may still be open.", detail: answer.message });
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
      {error ? (
        <Notice tone="crit" role="alert" testId="identity-pending-error">
          <span className="identity-notice__title">{error.sentence}</span>
          <RefusalDetails detail={error.detail} />
        </Notice>
      ) : null}
    </section>
  );
}

/**
 * Who asked for an agent and when, and how it runs, as its record says: a
 * record, not a mark on the agent. The spec digest and revision stay in
 * Technical details.
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
                      ? requester.name
                      : requester.words,
                }
              : null,
            provenance ? { label: "When", value: <time dateTime={new Date(provenance.atMs).toISOString()}>{formatAbsoluteTime(provenance.atMs)}</time> } : null,
            provenance?.consentForm ? { label: "Consent", value: consentWords(provenance.consentForm) } : null,
            // How the agent runs, as its record says; who reaches it is
            // the status's word, in the pane's overview.
            { label: "How it runs", value: runModeWords(record.runtimeMode) },
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
            // What is open for the agent, by id: the count is the record's
            // own line above.
            ...record.openOperations.map((operationId) => ({ label: "Open operation", value: operationId })),
          ]}
        />
      </div>
    </section>
  );
}
