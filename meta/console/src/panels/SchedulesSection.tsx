import React from "react";

import {
  CONSOLE_SCHEDULE_COMMANDS,
  describeSchedulePhase,
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

interface ScheduleForm {
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
  const [confirmingDelete, setConfirmingDelete] = React.useState<string | null>(null);
  const [form, setForm] = React.useState<ScheduleForm | null>(null);
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
    setForm((current) => {
      if (!current) return current;
      const weekdays = current.draft.weekdays.includes(day)
        ? current.draft.weekdays.filter((entry) => entry !== day)
        : [...current.draft.weekdays, day];
      return { ...current, draft: { ...current.draft, weekdays }, error: null };
    });
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
  return (
    <section
      className="console-schedules"
      aria-labelledby={headingId}
      data-testid={`schedules:${identity}`}
    >
      <div className="console-panel__header">
        <h4 id={headingId}>Schedules</h4>
        <div className="console-panel__actions">
          <button
            type="button"
            data-testid={`schedules-refresh:${identity}`}
            onClick={() => void load()}
          >
            Refresh
          </button>
          {editing.canChange && !form ? (
            <button
              type="button"
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
        <p className="console-schedules__error" role="alert" data-testid={`schedules-load-error:${identity}`}>
          {loadError}
        </p>
      ) : null}
      {actionError ? (
        <p className="console-schedules__error" role="alert" data-testid={`schedules-error:${identity}`}>
          {actionError}
        </p>
      ) : null}
      {form && editing.canChange ? renderForm(form) : null}
      {status === "loading" && schedules.length === 0 ? <p role="status">Loading schedules...</p> : null}
      {status === "ready" && schedules.length === 0 ? (
        <p className="console-schedules__empty" data-testid={`schedules-empty:${identity}`}>
          No schedules for this agent.
        </p>
      ) : null}
      {schedules.length > 0 ? (
        <ul className="console-panel__list console-schedules__list">
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
          <span className="console-schedules__phase" data-testid={`schedule-phase:${id}`}>
            {describeSchedulePhase(schedule.phase)}
          </span>
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
          <p className="console-schedules__meta" data-testid={`schedule-sender:${id}`}>
            {schedule.sender ? `Sends as ${schedule.sender}` : "Sends as this console"}
          </p>
        )}
        {editing.canChange ? (
          <div className="console-panel__actions">
            <button
              type="button"
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
                  aria-label={`Confirm deleting ${label}`}
                  data-testid={`schedule-delete-confirm:${id}`}
                  disabled={working}
                  onClick={() => runAction(schedule, CONSOLE_SCHEDULE_COMMANDS.delete)}
                >
                  Confirm delete
                </button>
                <button
                  type="button"
                  data-testid={`schedule-delete-cancel:${id}`}
                  onClick={() => setConfirmingDelete(null)}
                >
                  Keep
                </button>
              </>
            ) : (
              <button
                type="button"
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

  function renderForm(current: ScheduleForm) {
    const { draft, schedule } = current;
    const saving = busy === "form";
    return (
      <form
        className="console-schedules__form"
        data-testid={`schedule-form:${identity}`}
        aria-label={schedule ? "Edit schedule" : "New schedule"}
        onSubmit={submit}
      >
        <label className="console-schedules__field">
          Name (optional)
          <input
            data-testid="schedule-form-name"
            value={draft.name}
            onChange={(event) => change({ name: event.target.value })}
          />
        </label>
        <label className="console-schedules__field">
          Message
          <textarea
            data-testid="schedule-form-content"
            rows={3}
            value={draft.content}
            onChange={(event) => change({ content: event.target.value })}
          />
        </label>
        <label className="console-schedules__field">
          When
          <select
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
        </label>
        {draft.timing === "once" ? (
          <label className="console-schedules__field">
            At
            <input
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
                <select
                  data-testid="schedule-form-unit"
                  value={draft.unit}
                  onChange={(event) => change({ unit: event.target.value as ConsoleScheduleIntervalUnit })}
                >
                  <option value="minutes">minutes</option>
                  <option value="hours">hours</option>
                  <option value="days">days</option>
                </select>
              </label>
            </div>
            <label className="console-schedules__field">
              First send (optional; one interval from now when blank)
              <input
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
                    onChange={() => toggleWeekday(day)}
                  />
                  {label}
                </label>
              ))}
            </fieldset>
            <label className="console-schedules__field">
              Time zone
              <input
                data-testid="schedule-form-timezone"
                value={draft.timezone}
                onChange={(event) => change({ timezone: event.target.value })}
              />
            </label>
          </>
        ) : null}
        <p className="console-schedules__hint">
          Each send goes to {identity} as you, through the same checks as a message you send now.
        </p>
        {current.error ? (
          <p className="console-schedules__error" role="alert" data-testid="schedule-form-error">
            {current.error}
          </p>
        ) : null}
        <div className="console-panel__actions">
          <button type="submit" data-testid="schedule-form-save" disabled={busy !== null}>
            {saving ? "Saving..." : schedule ? "Save" : "Create schedule"}
          </button>
          <button
            type="button"
            data-testid="schedule-form-cancel"
            onClick={() => setForm(null)}
          >
            Cancel
          </button>
        </div>
      </form>
    );
  }
}
