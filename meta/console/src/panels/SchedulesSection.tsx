import React from "react";

import {
  CONSOLE_SCHEDULE_COMMANDS,
  describeSchedulePhase,
  describeScheduleSender,
  describeScheduleTrigger,
  emptyScheduleDraft,
  formatScheduleTime,
  parseConsoleScheduleList,
  scheduleCreateParams,
  scheduleDraftFromSchedule,
  scheduleErrorMessage,
  scheduleIsGone,
  scheduleUpdateParams,
  viewerTimeZone,
  type ConsoleSchedule,
  type ConsoleScheduleCommand,
  type ConsoleScheduleDraft,
  type ConsoleScheduleEditing,
  type ConsoleScheduleIntervalUnit,
  type ConsoleScheduleTimingKind,
} from "../lib/schedules";
import {
  SCHEDULE_RUN_WINDOW_MS,
  formatRunTime,
  nextPlannedRun,
  planScheduleRuns,
  type ScheduleKind,
  type ScheduleRunPlan,
} from "../lib/schedule-runs";
import {
  Chip,
  Countdown,
  DotStrip,
  EmptyState,
  KitGlyph,
  LoadingState,
  Notice,
  useAgentDirectory,
  type DotShape,
  type DotStripItem,
} from "../../../packages/console-components/src/panel-kit";

/// The least time between two list reads the pane's refreshes cause: the
/// list reads the whole schedule store, and the pane refreshes on activity.
const AUTO_REFRESH_INTERVAL_MS = 15_000;

const WEEKDAYS: Array<{ day: number; label: string }> = [
  { day: 1, label: "Mon" },
  { day: 2, label: "Tue" },
  { day: 3, label: "Wed" },
  { day: 4, label: "Thu" },
  { day: 5, label: "Fri" },
  { day: 6, label: "Sat" },
  { day: 0, label: "Sun" },
];

/** The mark of each schedule kind on a time strip (spec B10): once = a
 * filled square, a repeat = a tick, a calendar = a diamond. */
export const SCHEDULE_KIND_SHAPE: Record<ScheduleKind, DotShape> = {
  once: "square",
  interval: "tick",
  calendar: "diamond",
  unknown: "dot",
};

/** A form draft with one weekday toggled. */
export function toggleDraftWeekday(draft: ConsoleScheduleDraft, day: number): ConsoleScheduleDraft {
  const weekdays = draft.weekdays.includes(day)
    ? draft.weekdays.filter((entry) => entry !== day)
    : [...draft.weekdays, day];
  return { ...draft, weekdays };
}

export interface ScheduleFormProps {
  /** The agent the schedule sends to (its id names the form's test id). */
  identity: string;
  /** The agent's name for the hint; the id otherwise. */
  agentName?: string;
  /** The schedule being edited; null for a new one. */
  schedule: ConsoleSchedule | null;
  draft: ConsoleScheduleDraft;
  error: string | null;
  /** A write is running: the save button waits. */
  busy: boolean;
  /** This form's save is the running write. */
  saving: boolean;
  onChange: (patch: Partial<ConsoleScheduleDraft>) => void;
  onToggleWeekday: (day: number) => void;
  onSubmit: (event: React.FormEvent) => void;
  onCancel: () => void;
  formatTime?: (iso: string) => string;
  /** Fields before the message (the Schedules panel's agent choice). */
  lead?: React.ReactNode;
  className?: string;
}

/**
 * The schedule form (create and edit): name, message, when (keep, once,
 * repeating, daily with weekdays and a time zone), the refusal of the form
 * itself, and Save or Cancel. Agent details shows it inline; the Schedules
 * panel shows it in its drawer.
 */
export function ScheduleForm({
  identity,
  agentName,
  schedule,
  draft,
  error,
  busy,
  saving,
  onChange: change,
  onToggleWeekday,
  onSubmit,
  onCancel,
  formatTime = formatScheduleTime,
  lead,
  className,
}: ScheduleFormProps): React.JSX.Element {
  return (
    <form
      className={["console-schedules__form", className].filter(Boolean).join(" ")}
      data-testid={`schedule-form:${identity}`}
      aria-label={schedule ? "Edit schedule" : "New schedule"}
      onSubmit={onSubmit}
    >
      {lead}
      <label className="console-schedules__field">
        Name (optional)
        <input
          className="cc-input"
          data-testid="schedule-form-name"
          value={draft.name}
          onChange={(event) => change({ name: event.target.value })}
        />
      </label>
      <label className="console-schedules__field">
        Message
        <textarea
          className="cc-input"
          data-testid="schedule-form-content"
          rows={3}
          value={draft.content}
          onChange={(event) => change({ content: event.target.value })}
        />
      </label>
      <label className="console-schedules__field">
        When
        <span className="cc-select">
          <select
            className="cc-select__control"
            data-testid="schedule-form-timing"
            value={draft.timing}
            onChange={(event) => change({ timing: event.target.value as ConsoleScheduleTimingKind })}
          >
            {schedule ? (
              <option value="keep">{`Keep: ${describeScheduleTrigger(schedule.trigger, formatTime)}`}</option>
            ) : null}
            <option value="once">Once</option>
            <option value="interval">Repeating</option>
            <option value="daily">Daily</option>
          </select>
          <KitGlyph name="chevron" className="cc-select__chevron" />
        </span>
      </label>
      {draft.timing === "once" ? (
        <label className="console-schedules__field">
          At
          <input
            className="cc-input"
            type="datetime-local"
            data-testid="schedule-form-at"
            value={draft.at}
            onChange={(event) => change({ at: event.target.value })}
          />
        </label>
      ) : null}
      {draft.timing === "interval" ? (
        <>
          <div className="console-schedules__inline">
            <label className="console-schedules__field">
              Every
              <input
                className="cc-input"
                type="number"
                min={1}
                step={1}
                data-testid="schedule-form-every"
                value={draft.every}
                onChange={(event) => change({ every: event.target.value })}
              />
            </label>
            <label className="console-schedules__field">
              Unit
              <span className="cc-select">
                <select
                  className="cc-select__control"
                  data-testid="schedule-form-unit"
                  value={draft.unit}
                  onChange={(event) => change({ unit: event.target.value as ConsoleScheduleIntervalUnit })}
                >
                  <option value="minutes">minutes</option>
                  <option value="hours">hours</option>
                  <option value="days">days</option>
                </select>
                <KitGlyph name="chevron" className="cc-select__chevron" />
              </span>
            </label>
          </div>
          <label className="console-schedules__field">
            First send (optional; one interval from now when blank)
            <input
              className="cc-input"
              type="datetime-local"
              data-testid="schedule-form-start"
              value={draft.start}
              onChange={(event) => change({ start: event.target.value })}
            />
          </label>
        </>
      ) : null}
      {draft.timing === "daily" ? (
        <>
          <label className="console-schedules__field">
            At
            <input
              className="cc-input"
              type="time"
              data-testid="schedule-form-time"
              value={draft.time}
              onChange={(event) => change({ time: event.target.value })}
            />
          </label>
          <fieldset className="console-schedules__days">
            <legend>On (every day when none is picked)</legend>
            {WEEKDAYS.map(({ day, label }) => (
              <label key={day}>
                <input
                  type="checkbox"
                  data-testid={`schedule-form-day:${day}`}
                  checked={draft.weekdays.includes(day)}
                  onChange={() => onToggleWeekday(day)}
                />
                {label}
              </label>
            ))}
          </fieldset>
          <label className="console-schedules__field">
            Time zone
            <input
              className="cc-input"
              data-testid="schedule-form-timezone"
              value={draft.timezone}
              onChange={(event) => change({ timezone: event.target.value })}
            />
          </label>
        </>
      ) : null}
      <p className="console-schedules__hint">
        Each send goes to {agentName || identity} as you, through the same checks as a message you send now.
      </p>
      {error ? (
        <Notice tone="crit" role="alert" className="console-schedules__error" testId="schedule-form-error">
          {error}
        </Notice>
      ) : null}
      <div className="cc-btn-row console-schedules__actions">
        <button type="submit" className="cc-btn cc-btn--primary" data-testid="schedule-form-save" disabled={busy}>
          {saving ? "Saving..." : schedule ? "Save" : "Create schedule"}
        </button>
        <button
          type="button"
          className="cc-btn cc-btn--ghost"
          data-testid="schedule-form-cancel"
          onClick={onCancel}
        >
          Cancel
        </button>
      </div>
    </form>
  );
}

/** The width of an element, followed as it resizes (a fallback until measured). */
export function useMeasuredWidth<T extends HTMLElement>(fallback: number): [React.RefObject<T | null>, number] {
  const ref = React.useRef<T | null>(null);
  const [width, setWidth] = React.useState(fallback);
  React.useEffect(() => {
    const element = ref.current;
    if (!element) return undefined;
    const measure = () => {
      const next = Math.round(element.getBoundingClientRect().width);
      if (next > 0) setWidth((current) => (current === next ? current : next));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, width];
}

/** A schedule's planned runs as strip marks: shaped by kind, the next run
 * in the accent, a paused schedule's runs faint. */
export function scheduleStripItems(plans: readonly ScheduleRunPlan[]): DotStripItem[] {
  const items: DotStripItem[] = [];
  for (const plan of plans) {
    const name = plan.schedule.name || "Untitled schedule";
    const paused = plan.schedule.phase !== "active";
    plan.runs.forEach((run, index) => {
      items.push({
        id: `${plan.schedule.scheduleId}:${index}`,
        t: run,
        shape: paused ? "hollow" : SCHEDULE_KIND_SHAPE[plan.kind],
        live: !paused && plan.next === run,
        faint: paused,
        label: `${name} · ${formatRunTime(run, plan.schedule)}${paused ? " (paused)" : ""}`,
      });
    });
  }
  return items;
}

export interface SchedulesSectionProps {
  /** The agent whose schedules the section lists. */
  identity: string;
  /** Whether the viewer may change them, and why not. */
  editing: ConsoleScheduleEditing;
  /** Runs one schedule command for `identity`; rejects with the console's typed error. */
  run: (command: ConsoleScheduleCommand, params: Record<string, unknown>) => Promise<unknown>;
  /**
   * A value that changes whenever the pane refreshes: the list is read
   * again, at most once every 15 seconds.
   */
  refreshToken?: unknown;
  now?: () => Date;
  timezone?: string;
  formatTime?: (iso: string) => string;
}

interface ScheduleFormState {
  /** The schedule being edited; null for a new one. */
  schedule: ConsoleSchedule | null;
  draft: ConsoleScheduleDraft;
  error: string | null;
}

/**
 * The schedules of one agent in its details pane: deferred messages the
 * console sends to the agent as the viewer who defined them. A viewer who
 * may change them creates, edits, pauses, resumes and deletes them here;
 * anyone else sees the list read-only. The gateway judges every change.
 * Above the list, the next run and a strip of the next seven days.
 */
export function SchedulesSection({
  identity,
  editing,
  run,
  refreshToken,
  now = () => new Date(),
  timezone,
  formatTime = formatScheduleTime,
}: SchedulesSectionProps): React.JSX.Element {
  const [schedules, setSchedules] = React.useState<ConsoleSchedule[]>([]);
  const [status, setStatus] = React.useState<"loading" | "ready" | "failed">("loading");
  const [loadError, setLoadError] = React.useState<string | null>(null);
  const [actionError, setActionError] = React.useState<string | null>(null);
  const [busy, setBusy] = React.useState<string | null>(null);
  // A sender that is an agent reads by its display name, a console identity
  // in words; other senders (the console's own principals) read as they are.
  const directory = useAgentDirectory();
  const senderLabel = (sender: string) => directory.lookup(sender)?.label;
  const [confirmingDelete, setConfirmingDelete] = React.useState<string | null>(null);
  const [form, setForm] = React.useState<ScheduleFormState | null>(null);
  const [stripRef, stripWidth] = useMeasuredWidth<HTMLDivElement>(480);
  const runRef = React.useRef(run);
  runRef.current = run;
  const nowRef = React.useRef(now);
  nowRef.current = now;
  const alive = React.useRef(true);
  const loadSeq = React.useRef(0);
  const lastLoadAt = React.useRef<number | null>(null);
  React.useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const load = React.useCallback(async () => {
    const seq = ++loadSeq.current;
    lastLoadAt.current = nowRef.current().getTime();
    try {
      const result = await runRef.current(CONSOLE_SCHEDULE_COMMANDS.list, {});
      if (!alive.current || seq !== loadSeq.current) return;
      setSchedules(parseConsoleScheduleList(result));
      setStatus("ready");
      setLoadError(null);
    } catch (error) {
      if (!alive.current || seq !== loadSeq.current) return;
      // A list that cannot be read now is not shown as it was: the viewer
      // may have lost sight of the agent.
      setSchedules([]);
      setStatus("failed");
      setLoadError(scheduleErrorMessage(error, CONSOLE_SCHEDULE_COMMANDS.list));
    }
  }, []);

  React.useEffect(() => {
    void load();
  }, [identity, load]);

  React.useEffect(() => {
    const last = lastLoadAt.current;
    if (last !== null && nowRef.current().getTime() - last < AUTO_REFRESH_INTERVAL_MS) return;
    void load();
  }, [refreshToken, load]);

  React.useEffect(() => {
    if (!editing.canChange) {
      setForm(null);
      setConfirmingDelete(null);
    }
  }, [editing.canChange]);

  // The planned runs of the next seven days, from the list as read.
  const nowMs = now().getTime();
  const nowMinute = Math.floor(nowMs / 60_000) * 60_000;
  const plans = React.useMemo(
    () => schedules.map((schedule) => planScheduleRuns(schedule, nowMinute, SCHEDULE_RUN_WINDOW_MS)),
    [schedules, nowMinute],
  );
  const next = nextPlannedRun(plans);
  const stripItems = React.useMemo(() => scheduleStripItems(plans), [plans]);

  async function write(
    command: ConsoleScheduleCommand,
    params: Record<string, unknown>,
    key: string,
  ): Promise<boolean> {
    setBusy(key);
    setActionError(null);
    let done = false;
    try {
      await runRef.current(command, params);
      done = true;
    } catch (error) {
      if (alive.current) {
        setActionError(scheduleErrorMessage(error, command));
        if (scheduleIsGone(error)) {
          setForm(null);
          setConfirmingDelete(null);
        }
      }
    }
    if (alive.current) setBusy(null);
    await load();
    return done;
  }

  function openNew() {
    setActionError(null);
    setConfirmingDelete(null);
    setForm({ schedule: null, draft: emptyScheduleDraft(now(), timezone ?? viewerTimeZone()), error: null });
  }

  function openEdit(schedule: ConsoleSchedule) {
    setActionError(null);
    setConfirmingDelete(null);
    setForm({
      schedule,
      draft: scheduleDraftFromSchedule(schedule, now(), timezone ?? viewerTimeZone()),
      error: null,
    });
  }

  function change(patch: Partial<ConsoleScheduleDraft>) {
    setForm((current) => (current ? { ...current, draft: { ...current.draft, ...patch }, error: null } : current));
  }

  function toggleWeekday(day: number) {
    setForm((current) => (current ? { ...current, draft: toggleDraftWeekday(current.draft, day), error: null } : current));
  }

  function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!form || busy) return;
    const request = form.schedule
      ? scheduleUpdateParams(form.schedule, form.draft, now())
      : scheduleCreateParams(form.draft, now());
    if ("error" in request) {
      setForm({ ...form, error: request.error });
      return;
    }
    const command = form.schedule ? CONSOLE_SCHEDULE_COMMANDS.update : CONSOLE_SCHEDULE_COMMANDS.create;
    void write(command, request.params, "form").then((done) => {
      if (done && alive.current) setForm(null);
    });
  }

  function runAction(schedule: ConsoleSchedule, command: ConsoleScheduleCommand) {
    if (busy) return;
    void write(command, { schedule_id: schedule.scheduleId }, `${schedule.scheduleId}:${command}`).then((done) => {
      if (done && alive.current && command === CONSOLE_SCHEDULE_COMMANDS.delete) {
        setConfirmingDelete(null);
        setForm((current) => (current?.schedule?.scheduleId === schedule.scheduleId ? null : current));
      }
    });
  }

  const headingId = `schedules-heading:${identity}`;
  const agentName = directory.lookup(identity)?.label;
  return (
    <section
      className="console-schedules"
      aria-labelledby={headingId}
      data-testid={`schedules:${identity}`}
    >
      <div className="cc-section-head console-schedules__head">
        <h3 id={headingId} className="cc-section-head__label">Schedules</h3>
        <div className="cc-section-head__action">
          <button
            type="button"
            className="cc-btn cc-btn--sm cc-btn--ghost"
            data-testid={`schedules-refresh:${identity}`}
            onClick={() => void load()}
          >
            Refresh
          </button>
          {editing.canChange && !form ? (
            <button
              type="button"
              className="cc-btn cc-btn--sm"
              data-testid={`schedules-new:${identity}`}
              onClick={openNew}
            >
              New schedule
            </button>
          ) : null}
        </div>
      </div>
      {editing.note ? (
        <p className="console-schedules__note" data-testid={`schedules-note:${identity}`}>
          {editing.note}
        </p>
      ) : null}
      {loadError ? (
        <Notice tone="crit" role="alert" className="console-schedules__error" testId={`schedules-load-error:${identity}`}>
          {loadError}
        </Notice>
      ) : null}
      {actionError ? (
        <Notice tone="crit" role="alert" className="console-schedules__error" testId={`schedules-error:${identity}`}>
          {actionError}
        </Notice>
      ) : null}
      {schedules.length > 0 ? (
        <div className="console-schedules__glance" ref={stripRef}>
          <p className="console-schedules__next" data-testid={`schedules-next:${identity}`}>
            {next ? (
              <>
                <span className="console-schedules__next-label">Next</span>
                <span className="console-schedules__next-name">{next.plan.schedule.name || "Untitled schedule"}</span>
                <Countdown to={next.at} now={nowMs} />
              </>
            ) : (
              <span className="console-schedules__next-label">No run planned</span>
            )}
          </p>
          <div className="console-schedules__strip">
            <span className="console-schedules__strip-label" aria-hidden="true">Next 7 days</span>
            <DotStrip
              items={stripItems}
              start={nowMinute}
              end={nowMinute + SCHEDULE_RUN_WINDOW_MS}
              width={Math.max(160, stripWidth)}
              height={18}
              axis
              now={nowMs}
              label={`Next 7 days: ${stripItems.filter((item) => !item.faint).length} planned runs`}
              testIdPrefix={`schedule-run:${identity}`}
              testId={`schedules-strip:${identity}`}
            />
          </div>
        </div>
      ) : null}
      {form && editing.canChange ? (
        <ScheduleForm
          identity={identity}
          agentName={agentName}
          schedule={form.schedule}
          draft={form.draft}
          error={form.error}
          busy={busy !== null}
          saving={busy === "form"}
          onChange={change}
          onToggleWeekday={toggleWeekday}
          onSubmit={submit}
          onCancel={() => setForm(null)}
          formatTime={formatTime}
        />
      ) : null}
      {status === "loading" && schedules.length === 0 ? <LoadingState label="Loading schedules" rows={2} /> : null}
      {status === "ready" && schedules.length === 0 ? (
        <EmptyState inline title="No schedules for this agent." testId={`schedules-empty:${identity}`} />
      ) : null}
      {schedules.length > 0 ? (
        <ul className="console-schedules__list">
          {schedules.map(renderSchedule)}
        </ul>
      ) : null}
    </section>
  );

  function renderSchedule(schedule: ConsoleSchedule) {
    const id = schedule.scheduleId;
    const label = schedule.name || "Untitled schedule";
    const working = busy !== null;
    return (
      <li key={id} className="console-schedules__item" data-testid={`schedule:${id}`} data-phase={schedule.phase}>
        <div className="console-schedules__row">
          <strong>{label}</strong>
          <Chip
            className="console-schedules__phase"
            tone={schedule.phase === "active" ? "ok" : schedule.phase === "paused" ? "warn" : "neutral"}
            mark
            testId={`schedule-phase:${id}`}
          >
            {describeSchedulePhase(schedule.phase)}
          </Chip>
        </div>
        <p className="console-schedules__content">{schedule.content}</p>
        <p className="console-schedules__meta" data-testid={`schedule-when:${id}`}>
          {describeScheduleTrigger(schedule.trigger, formatTime)}
        </p>
        {schedule.phase === "active" ? (
          <p className="console-schedules__meta" data-testid={`schedule-next:${id}`}>
            {schedule.nextDueAt ? `Next: ${formatTime(schedule.nextDueAt)}` : "Next: none planned"}
          </p>
        ) : null}
        {schedule.senderHidden ? null : (
          <p
            className="console-schedules__meta"
            data-testid={`schedule-sender:${id}`}
            title={schedule.sender || undefined}
          >
            {`Sends as ${describeScheduleSender(schedule.sender, senderLabel)}`}
          </p>
        )}
        {editing.canChange ? (
          <div className="cc-btn-row console-schedules__actions">
            <button
              type="button"
              className="cc-btn cc-btn--sm"
              aria-label={`Edit ${label}`}
              data-testid={`schedule-edit:${id}`}
              disabled={working}
              onClick={() => openEdit(schedule)}
            >
              Edit
            </button>
            {schedule.phase === "active" ? (
              <button
                type="button"
                className="cc-btn cc-btn--sm"
                aria-label={`Pause ${label}`}
                data-testid={`schedule-pause:${id}`}
                disabled={working}
                onClick={() => runAction(schedule, CONSOLE_SCHEDULE_COMMANDS.pause)}
              >
                Pause
              </button>
            ) : null}
            {schedule.phase === "paused" ? (
              <button
                type="button"
                className="cc-btn cc-btn--sm"
                aria-label={`Resume ${label}`}
                title="Resuming makes you its sender"
                data-testid={`schedule-resume:${id}`}
                disabled={working}
                onClick={() => runAction(schedule, CONSOLE_SCHEDULE_COMMANDS.resume)}
              >
                Resume
              </button>
            ) : null}
            {confirmingDelete === id ? (
              <>
                <button
                  type="button"
                  className="cc-btn cc-btn--sm cc-btn--danger"
                  aria-label={`Confirm deleting ${label}`}
                  data-testid={`schedule-delete-confirm:${id}`}
                  disabled={working}
                  onClick={() => runAction(schedule, CONSOLE_SCHEDULE_COMMANDS.delete)}
                >
                  Confirm delete
                </button>
                <button
                  type="button"
                  className="cc-btn cc-btn--sm cc-btn--ghost"
                  data-testid={`schedule-delete-cancel:${id}`}
                  onClick={() => setConfirmingDelete(null)}
                >
                  Keep
                </button>
              </>
            ) : (
              <button
                type="button"
                className="cc-btn cc-btn--sm cc-btn--danger"
                aria-label={`Delete ${label}`}
                data-testid={`schedule-delete:${id}`}
                disabled={working}
                onClick={() => setConfirmingDelete(id)}
              >
                Delete
              </button>
            )}
          </div>
        ) : null}
      </li>
    );
  }
}
