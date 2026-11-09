import { captureGuideSelection, guideSelectionAvailable, guideSelectionEvents } from "./guide-selection.mjs";
import React, { useEffect, useRef, useState } from "react";
import { request } from "./state.mjs";
import { createGcpPreflightGate, gcpPreflightAvailable, gcpPreflightScope, gcpPreflightUrl,
  gcpBindingMatches, gcpOptionsValid, gcpProjectsValid, gcpCheckValid, gcpCheckFresh, GCP_PREFLIGHT_FRESH_MS } from "./gcp-preflight.mjs";
import "./gcp-preflight.css";

// The server bounds one whole cloud access check at 180 s (PROBE_TIMEOUT in
// launchkit/gcp_preflight.py): it reads and checks the accepted package before
// and after the account reads. It refuses another check until this one ends,
// and past its bound it answers with its own refusal. Keep the read open a
// minute longer than that bound, not the ordinary 30 s, so the page shows the
// server's answer instead of giving up on a check that is still running.
const CLOUD_ACCESS_READ_TIMEOUT_MS = 240000;

function CloudChecks({ checks = [] }) {
  return checks.length ? <ul className="gcp-preflight-checks">{checks.map((check) => <li key={check.id} data-check-status={check.status}>
    <div><strong>{check.label}</strong><span>{check.status === "checked" ? "Checked" : check.status === "blocked" ? "Needs attention" : "Not verified"}</span></div>
    <p>{check.detail}</p>{check.action && <p className="gcp-preflight-fix">{check.action}</p>}
  </li>)}</ul> : null;
}

function CloudPlan({ value, guided = false }) {
  return <div className="gcp-preflight-plan">
    <span className="setup-eyebrow">DESTINATION PLAN</span><h4>{value.title}</h4>
    <p>{guided ? "The observed preconditions support this destination plan for the accepted package. Computer preparation and application startup will be checked separately." : "The observed preconditions support this destination preview for the accepted package. Provisioning and application startup remain unverified."}</p>
    {guided && <p>The guided path creates the listed VM, boot disk and SSH firewall rule. It prepares Python and user-service support, publishes the new host key and uses a private SSH identity on this computer. VM and disk charges apply until you explicitly remove the VM and disk.</p>}
    <dl><div><dt>Project</dt><dd>{value.destination.project}</dd></div><div><dt>Zone</dt><dd>{value.destination.zone}</dd></div>
      <div><dt>Computer</dt><dd>{value.destination.machine_type}</dd></div><div><dt>Disk</dt><dd>{value.destination.disk_gb} GB</dd></div></dl>
    {[["Resources", value.resources], ["Access", value.access], ["Changes in the deployment", value.mutations]].map(([label, entries]) =>
      entries.length ? <div key={label}><h5>{label}</h5><ul>{entries.map((entry, i) => <li key={i}>{entry}</li>)}</ul></div> : null)}
    {value.limitations.length > 0 && <div><h5>Remaining verification</h5><ul>{value.limitations.map((entry, i) => <li key={i}>{entry}</li>)}</ul></div>}
    {value.commands.length > 0 && <details><summary>Commands for this plan (advanced)</summary>{value.commands.map((command, i) => <pre key={i}>{command}</pre>)}</details>}
  </div>;
}

export function GcpPreflight({ state, build, blocked = false, onChooseProject, onDownload, downloading = false, onProvision, hasSavedRequest = false, selectionScope }) {
  const selectionToken = selectionScope?.token();
  const selectionBlocked = !guideSelectionAvailable(selectionScope, state, selectionToken);
  const gate = useRef(null), latest = useRef(null), activeRequest = useRef(null), selection = useRef({ scope: "", account: "", project: "" });
  if (!gate.current) gate.current = createGcpPreflightGate();
  const scope = gcpPreflightScope(state, build, blocked), scopeRevision = gate.current.observe(scope), epoch = gate.current.capture().epoch;
  if (selection.current.scope !== scope || selection.current.revision !== scopeRevision) selection.current = { scope, revision: scopeRevision, account: "", project: "" };
  const [choice, setChoice] = useState(selection.current);
  const [optionsRecord, setOptions] = useState(null), [projectsRecord, setProjects] = useState(null), [resultRecord, setResult] = useState(null);
  const [busyRecord, setBusy] = useState(null), [errorRecord, setError] = useState(null), [projectDraft, setProjectDraft] = useState("");
  const [, setFreshness] = useState(0);
  const available = gcpPreflightAvailable(state, build, blocked);
  latest.current = { state, build, blocked, scope, available };
  const currentChoice = choice.scope === scope && choice.revision === scopeRevision ? choice : selection.current;
  const options = optionsRecord?.scope === scope && optionsRecord.revision === scopeRevision ? optionsRecord.value : null;
  const projects = projectsRecord?.scope === scope && projectsRecord.revision === scopeRevision && projectsRecord.account === currentChoice.account ? projectsRecord.value : null;
  const result = resultRecord?.scope === scope && resultRecord.epoch === epoch ? resultRecord.value : null;
  const busy = busyRecord?.scope === scope && busyRecord.epoch === epoch ? busyRecord.value : "";
  const error = errorRecord?.scope === scope && errorRecord.epoch === epoch ? errorRecord.value : "";
  const fresh = result && gcpCheckFresh(result);

  useEffect(() => {
    setChoice(selection.current); setOptions(null); setProjects(null); setResult(null); setBusy(null); setError(null);
    setProjectDraft((state.proposal || state.blueprint)?.gcp?.project || "");
  }, [scope]);
  useEffect(() => { gate.current.open(); return () => gate.current.close(); }, []);
  useEffect(() => {
    if (!result || !gcpCheckFresh(result)) return;
    const selected = gate.current.capture(), delay = Math.max(1, Date.parse(result.checked_at) + GCP_PREFLIGHT_FRESH_MS - Date.now());
    const timer = setTimeout(() => { if (gate.current.current(selected)) setFreshness((value) => value + 1); }, delay);
    return () => clearTimeout(timer);
  }, [resultRecord, scope]);

  function begin(kind, nextChoice = selection.current) {
    const ownerCurrent = captureGuideSelection(selectionScope, state, selectionToken);
    if (!ownerCurrent) return null;
    gate.current.invalidate();
    selection.current = { ...nextChoice, scope: latest.current.scope, revision: gate.current.capture().revision };
    setChoice(selection.current); setResult(null); setError(null); setBusy(null);
    const token = gate.current.capture();
    activeRequest.current = kind ? { ...token, value: kind } : null;
    setBusy({ ...token, value: kind });
    return { ...token, ownerCurrent, state: latest.current.state, build: latest.current.build, choice: { ...selection.current } };
  }
  function owns(selected) {
    return !!selected && selected.ownerCurrent() && gate.current.current(selected) && latest.current.available && selection.current.account === selected.choice.account &&
      selection.current.project === selected.choice.project;
  }
  function accept(selected, response, expectedBinding = null) {
    if (!owns(selected)) return false;
    if (!gcpBindingMatches(response, latest.current.state, latest.current.build, expectedBinding))
      throw new Error("Cloud checks do not match this accepted package. Check cloud access again.");
    return true;
  }
  function finish(selected, failure = null) {
    if (!selected) return;
    if (failure && owns(selected)) setError({ ...selected, value: failure.message || "Cloud access could not be checked. Try again." });
    if (gate.current.current(selected) && activeRequest.current?.epoch === selected.epoch) {
      activeRequest.current = null; setBusy(null);
    }
  }
  const currentBusy = () => activeRequest.current && gate.current.current(activeRequest.current) ? activeRequest.current.value : "";

  async function loadOptions() {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (!latest.current.available || currentBusy()) return;
    setOptions(null); setProjects(null);
    const selected = begin("options", { account: "", project: "" });
    try {
      const value = await request(`${gcpPreflightUrl(selected.state.id, "options")}?${new URLSearchParams({ build_id: selected.build.build_id })}`,
        { timeoutMs: CLOUD_ACCESS_READ_TIMEOUT_MS });
      if (!accept(selected, value)) return;
      if (!gcpOptionsValid(value)) throw new Error("Cloud access returned incomplete account information. Check again.");
      setOptions({ scope: selected.scope, revision: selected.revision, value });
    } catch (failure) { finish(selected, failure); }
    finally { finish(selected); }
  }
  async function chooseAccount(account) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (!latest.current.available || !options?.accounts.some((item) => item.id === account)) return;
    if (currentBusy() === "projects" && selection.current.account === account && !selection.current.project) return;
    setProjects(null);
    const selected = begin("projects", { account, project: "" }), binding = options.binding;
    try {
      const value = await request(gcpPreflightUrl(selected.state.id, "projects"), { method: "POST", body: {
        version: selected.state.version, build_id: selected.build.build_id, account } });
      if (!accept(selected, value, binding)) return;
      if (!gcpProjectsValid(value, account)) throw new Error("The project list could not be verified for this account. Retry or enter a project ID.");
      setProjects({ scope: selected.scope, revision: selected.revision, account, value });
    } catch (failure) { finish(selected, failure); }
    finally { finish(selected); }
  }
  function chooseProject(project) {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (!latest.current.available || !selection.current.account) return;
    const selected = begin("", { account: selection.current.account, project });
    setProjectDraft(project); finish(selected);
  }
  async function checkDestination() {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (!latest.current.available || currentBusy() || !options || !selection.current.account || !selection.current.project) return;
    const selected = begin("check"), binding = options.binding;
    try {
      const value = await request(gcpPreflightUrl(selected.state.id, onProvision || hasSavedRequest ? "check-guided" : "check"), { method: "POST", body: {
        version: selected.state.version, build_id: selected.build.build_id,
        account: selected.choice.account, project: selected.choice.project } });
      if (!accept(selected, value, binding)) return;
      if (!gcpCheckValid(value, selected.choice.account, selected.choice.project)) throw new Error("Destination checks returned incomplete evidence. Check the destination again.");
      setResult({ scope: selected.scope, epoch: selected.epoch, value });
    } catch (failure) { finish(selected, failure); }
    finally { finish(selected); }
  }
  async function useProject() {
    if (!guideSelectionAvailable(selectionScope, state, selectionToken)) return;
    if (!latest.current.available || currentBusy() || !result || !onChooseProject || result.project === result.destination.project) return;
    const selected = begin("project");
    try {
      await onChooseProject({ scope: selected.scope, project: selected.choice.project, isCurrent: () => owns(selected) });
    } catch (failure) { finish(selected, failure); }
    finally { finish(selected); }
  }

  if ((state.proposal || state.blueprint)?.target !== "gcp") return null;
  const proposedProject = (state.proposal || state.blueprint)?.gcp?.project;
  return <section aria-disabled={selectionBlocked} {...guideSelectionEvents(selectionScope, state, selectionToken)} className="gcp-preflight" aria-label="Google Cloud setup" aria-busy={!!busy}>
    <span className="setup-eyebrow">GUIDED CLOUD SETUP</span><h3>Choose where your agents will live</h3>
    <p>We will check the Google Cloud accounts on this computer, choose a project, and explain what this application needs there.</p>
    {!available && <p className="deployment-note">{blocked ? "Finish the current design change before checking this destination." : "Accept a current application build to check its cloud destination."}</p>}
    {!options && <button type="button" className="setup-action primary" disabled={selectionBlocked || !available || !!busy} onClick={loadOptions}>{busy === "options" ? "Checking cloud access..." : "Check cloud access"}</button>}
    {options && <>
      {!options.cli.available && <div role="status"><p>{options.cli.detail}</p>{options.cli.action && <p className="gcp-preflight-fix">{options.cli.action}</p>}</div>}
      <CloudChecks checks={options.checks} />
      {options.accounts.length > 0 && <fieldset className="gcp-preflight-choices"><legend>Which account should we use?</legend><p>Choose an account already signed in on this computer.</p>
        {options.accounts.map((account) => <button key={account.id} type="button" aria-pressed={currentChoice.account === account.id} disabled={selectionBlocked || !available}
          onClick={() => chooseAccount(account.id)}><strong>{account.label}</strong><small>{account.active ? "Current account" : account.id}</small></button>)}
      </fieldset>}
      {options.accounts.length === 0 && options.cli.available && <p>No account choices are available from this check. Review the access checks above, then check again.</p>}
      {currentChoice.account && <fieldset className="gcp-preflight-choices"><legend>Which project should host the application?</legend>
        <p>{busy === "projects" ? "Finding projects this account can access..." : "Pick a project, or enter its exact ID below."}</p>
        {projects?.projects.map((project) => <button key={project.id} type="button" aria-pressed={currentChoice.project === project.id} disabled={selectionBlocked || !available}
          onClick={() => chooseProject(project.id)}><strong>{project.label}</strong><small>{project.id}{project.id === proposedProject ? " - Proposed destination" : ""}</small></button>)}
        {proposedProject && !projects?.projects.some((project) => project.id === proposedProject) && <button type="button" aria-pressed={currentChoice.project === proposedProject} disabled={selectionBlocked || !available}
          onClick={() => chooseProject(proposedProject)}><strong>Use the proposed project</strong><small>{proposedProject} - Access still needs checking</small></button>}
        {projects?.truncated && <p className="deployment-note">This project list is incomplete. Enter an exact ID to check another project.</p>}
        <CloudChecks checks={projects?.checks} />
        <details className="gcp-preflight-project-id"><summary>Enter a project ID</summary><label>Google Cloud project ID<input name="gcp_project_id" value={projectDraft} disabled={selectionBlocked || !available}
          onChange={(event) => { const selected = begin("", { account: selection.current.account, project: "" }); if (!selected) return; setProjectDraft(event.target.value); finish(selected); }} placeholder="my-agent-project" /></label>
          <button type="button" className="setup-action" disabled={selectionBlocked || !available || !/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/.test(projectDraft)} onClick={() => chooseProject(projectDraft)}>Choose this project</button></details>
      </fieldset>}
      {currentChoice.project && <div className="gcp-preflight-destination"><p>Check <strong>{currentChoice.project}</strong> using <strong>{currentChoice.account}</strong>.</p>
        <button type="button" className="setup-action primary" disabled={selectionBlocked || !available || !!busy} onClick={checkDestination}>{busy === "check" ? "Checking destination..." : result ? "Recheck destination" : "Check destination"}</button></div>}
      {result && <div className="gcp-preflight-result" role="status"><h4>{!fresh ? "These checks need a refresh" : result.ready_for_plan ? "Your destination plan is ready" : "Here is what needs attention"}</h4>
        {!fresh && <p>Cloud access can change. Recheck this destination to refresh its plan; the observations below are from the earlier check.</p>}<CloudChecks checks={result.checks} />
        {result.project !== result.destination.project && <div className="gcp-preflight-project-change"><p>The accepted application currently targets <strong>{result.destination.project || "an unspecified project"}</strong>. Using <strong>{result.project}</strong> changes your proposal and requires a fresh accepted build.</p>
          {onChooseProject && <button type="button" className="setup-action" disabled={selectionBlocked || !available || !!busy} onClick={useProject}>Use this project</button>}</div>}
        {fresh && result.ready_for_plan && <><CloudPlan value={result.plan} guided={!!onProvision || hasSavedRequest} />
          {onProvision && result.guided_supported === true && <div className="gcp-preflight-next"><button type="button" className="setup-action primary" disabled={selectionBlocked || !available || !!busy} onClick={() => {
            if (guideSelectionAvailable(selectionScope, state, selectionToken) && latest.current.available && gate.current.current(resultRecord) && gcpCheckFresh(resultRecord.value))
              onProvision({ value: structuredClone(resultRecord.value), isCurrent: () => guideSelectionAvailable(selectionScope, state, selectionToken) && latest.current.available && gate.current.current(resultRecord) && gcpCheckFresh(resultRecord.value) && gcpBindingMatches(resultRecord.value, latest.current.state, latest.current.build) });
          }}>{hasSavedRequest ? "Retry the original cloud setup request" : "Create this cloud computer"}</button></div>}
          {onDownload && <div className="gcp-preflight-next"><p>Get the accepted application package to continue with this destination plan.</p>
            <button type="button" className="setup-action primary" disabled={selectionBlocked || !available || !!busy || downloading} onClick={() => {
              if (guideSelectionAvailable(selectionScope, state, selectionToken) && latest.current.available && gate.current.current(resultRecord) && gcpCheckFresh(resultRecord.value))
                onDownload({ binding: structuredClone(result.binding), isCurrent: () =>
                  guideSelectionAvailable(selectionScope, state, selectionToken) && latest.current.available && gate.current.current(resultRecord) && gcpCheckFresh(resultRecord.value) &&
                  gcpBindingMatches(resultRecord.value, latest.current.state, latest.current.build) });
            }}>{downloading ? "Preparing accepted package..." : "Download accepted GCP package"}</button></div>}
        </>}<small>Checked {new Date(result.checked_at).toLocaleString()} for build {result.binding.build_id}.</small>
      </div>}
      <button type="button" className="setup-action gcp-preflight-retry" disabled={selectionBlocked || !available || !!busy} onClick={loadOptions}>Check cloud access again</button>
    </>}
    {error && <p className="deployment-error" role="alert">{error}</p>}
  </section>;
}
