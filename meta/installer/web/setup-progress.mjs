import { implementationIsCurrent, implementationIsRunning, implementationAcceptance } from './implementation-panel.mjs';
import { deploymentCanPrepare, deploymentConsole, nativeTeamCanInstall, deploymentSourceKind } from './deployment-panel.mjs';
import { rehearsalActive, rehearsalCurrent, rehearsalCanContinue, applicationTrialMatches, applicationTrialRepair,
  applicationTrialEndToRepair } from './rehearsal-panel.mjs';
import { inputKey } from './wizard-state.mjs';
import { packagePreparationReason, PACKAGE_REVISION_PROMPT } from './package-availability.mjs';
import { remoteReceiptOwned, remoteInstallAdmitted, remoteObservationAvailable, remoteNativeReady,
  remoteCloudRemoval, remotePhase } from './remote-deployment.mjs';

const action = (target, label) => ({ target, label });
const trialAction = (label, mode) => ({ ...action('trial', label), ...(mode ? { mode } : {}) });
const installAction = (label, sourceKind) => ({ ...action('deployment', label), sourceKind });
const buildObservationKnown = (flags) => flags.buildObservation === undefined || flags.buildObservation === 'known';
const deploymentObservationKnown = (flags) => flags.deploymentObservation === undefined || flags.deploymentObservation === 'known';

function ownedRemoteInstallation(receipt, guide) {
  return remoteReceiptOwned(receipt, guide) && (receipt.admitted === true ||
    receipt.remote_admission_requested === true || remoteInstallAdmitted(receipt));
}

/** Remote status directs the user to its existing controls, never to an inferred browser URL. */
function remoteInstallationProgress(receipt, guide, flags) {
  const owned = ownedRemoteInstallation(receipt, guide);
  const selectedTarget = (guide.proposal || guide.blueprint)?.target;
  if (!owned && (!['gcp', 'linux'].includes(selectedTarget) || flags.remoteObservation === undefined ||
      flags.remoteObservation === 'known')) return null;
  const label = receipt?.target === 'gcp' ? 'cloud' : 'remote';
  const observationKnown = flags.remoteObservation === undefined || flags.remoteObservation === 'known';
  if (!observationKnown || owned && !remoteObservationAvailable(receipt)) return {
    title: flags.remoteObservation === 'loading' ? 'Checking your remote installation status' : 'Remote installation status is unavailable',
    detail: 'Check the saved destination before starting another installation or using its application. Your task and setup drafts remain available.' +
      (owned ? ' Saved remote installation findings remain available.' : ''),
    actions: [action('remote', 'Review remote status')], attention: true,
  };
  const removal = remoteCloudRemoval(receipt);
  if (removal) return { title: removal === 'removed' ? 'Your cloud installation resources were removed' : 'Your cloud installation removal needs follow-up',
    detail: 'Review the saved removal request and resource findings. The previous application status does not establish usable access.',
    actions: [action('remote', 'Review cloud removal')], attention: true };
  if (receipt.status === 'running') return remoteNativeReady(receipt)
    ? { title: `Your installed ${label} application is running`,
      detail: 'Use the saved destination controls to check or open the browser connection and try your first job. Runtime readiness does not establish browser access or a useful job result.',
      actions: [action('remote', 'Use your remote application')] }
    : { title: 'Your remote runtime needs a readiness check',
      detail: 'A running service alone does not verify its source or health. Recheck the saved destination before using the application.',
      actions: [action('remote', 'Check remote readiness')], attention: true };
  if (receipt.status === 'stopped') return { title: `Your installed ${label} application is stopped`,
    detail: 'Use its saved destination controls to restart the installed source when available. Source matching and cleanup checks still apply; your drafts and installed records are kept.',
    actions: [action('remote', 'Manage remote application')] };
  if (['failed', 'recovery-needed', 'not-installed'].includes(receipt.status)) return {
    title: receipt.status === 'recovery-needed' ? 'Your remote installation needs cleanup verification' : 'Your remote installation needs attention',
    detail: 'Review the saved destination, failure details and available recovery controls. Preserve its folder and private settings; another installation requires the existing source and cleanup checks.',
    actions: [action('remote', 'Review remote recovery')], attention: true };
  return { title: remotePhase(receipt), detail: 'Follow the saved destination operation and its checks before trying the application.',
    actions: [action('remote', 'Follow remote installation')] };
}

export function nativeTeamPackagePlan(state) {
  if (!state?.id || !Number.isInteger(state.version) || state.operation?.status === "running") return null;
  const plan = state.proposal || state.blueprint;
  if (!plan?.project) return null;
  if (state.status === "ready" && !state.proposal) return plan;
  return state.readiness?.context_ready === true && Array.isArray(state.readiness.decision_blockers) &&
    state.readiness.decision_blockers.length === 0 ? plan : null;
}

/** Unfinished application work is the next step until this guide has a checked build. */
export function setupNeedsApplicationBuild(guide, build, flags = {}) {
  if (!buildObservationKnown(flags)) return false;
  const readiness = guide?.readiness;
  if (!guide?.id || !(guide.proposal || guide.blueprint) || readiness?.context_ready !== true ||
      !Array.isArray(readiness.implementation_blockers) || readiness.implementation_blockers.length === 0 ||
      (readiness.decision_blockers?.length || 0) > 0) return false;
  return !(deploymentCanPrepare(build, guide) && implementationAcceptance(build, guide)?.passed &&
    build.source_verified === true);
}


function ownedRunningBuild(guide, build) {
  return !!guide?.id && build?.guide_id === guide.id &&
    typeof build.build_id === 'string' && !!build.build_id && implementationIsRunning(build);
}

function activeBuildProgress(build, earlier = false) {
  if (build.cleanup_pending === true && build.cleanup_in_progress !== true && ['completed', 'failed', 'interrupted'].includes(build.status)) {
    return { title: 'Your build needs cleanup',
      detail: 'The build has finished, but its runtime has not finished closing. Saved source and verification reports remain available. Open the builder and retry cleanup before starting another build, application trial or installation.',
      actions: [action('build', 'Review cleanup')], attention: true };
  }
  if (build.cleanup_in_progress === true || build.stop_requested === true) {
    return { title: build.stop_requested === true ? 'Your builder is stopping' : 'Your build runtime is closing',
      detail: 'The builder is closing its runtime. Its saved source is preserved. Follow cleanup below; new builds, application trials and installations will be available after cleanup is confirmed.',
      actions: [action('build', 'Follow cleanup')] };
  }
  if (earlier) return { title: 'An earlier proposal is still building',
    detail: 'Your setup changed while this build was working. Follow its saved activity and stop it before building the updated proposal. Its source remains separate from your current design.',
    actions: [action('build', 'Follow earlier build')], attention: true };
  return { title: 'Your application is being built and checked',
    detail: 'The builder and independent reviewer show their actual activity below.',
    actions: [action('build', 'Follow the build')] };
}

export const GUIDE_RESUME_PROMPT = 'Review the actual saved build, trial and installation evidence for this setup. Explain what is implemented, what was tested in isolation, what is running, and what still needs a decision or verification. Recommend the next useful decision or action while preserving my requirements and previous choices. Ask a context-aware multiple-choice question when useful. Do not infer that my real job succeeded from an agent turn, an offline test, or runtime startup.';
export const SETUP_DISCOVERY_PROMPT = 'Continue setup from my saved proposal, brief, earlier answers and manual edits. Preserve my confirmed choices and manual changes. Ask the next useful context-derived multiple-choice question about what is still unresolved. Do not re-answer earlier questions or treat provisional defaults as confirmed choices.';

function pausedSetupDiscovery(guide) {
  return guide.status === 'refining' && guide.question == null && guide.readiness?.context_ready === false &&
    (!guide.operation || guide.operation.status === 'completed');
}

/** Lifecycle narration belongs to this setup, not an unrelated saved prompt. */
export function setupProgressHasLifecycle(guide, { build, trial, deployment, remoteDeployment } = {}) {
  return !!guide?.id && !!(
    implementationIsCurrent(build, guide) && typeof build?.build_id === 'string' && build.build_id || ownedRunningBuild(guide, build) ||
    trial?.guide_id === guide.id && typeof trial.rehearsal_id === 'string' && trial.rehearsal_id ||
    deployment?.guide_id === guide.id && typeof deployment.deployment_id === 'string' && deployment.deployment_id ||
    ownedRemoteInstallation(remoteDeployment, guide));
}

/** Resume saved discovery or actual lifecycle progress only on an explicit click. */
export function setupGuideResume(guide, receipts = {}, flags = {}) {
  if (!guide?.id || !Number.isSafeInteger(guide.version) || flags.setupLoading || flags.processing || flags.packageBusy ||
      flags.active === false || guide.operation?.status === 'running') return null;
  const progress = setupProgress(guide, receipts, flags);
  if (!progress) return null;
  const text = progress.kind === 'package-pending' ? PACKAGE_REVISION_PROMPT : progress.kind === 'discovery' ? SETUP_DISCOVERY_PROMPT
    : setupProgressHasLifecycle(guide, receipts) ? GUIDE_RESUME_PROMPT : null;
  return text ? { guide_id: guide.id, guide_version: guide.version, text } : null;
}

/** Recheck the clicked guide/version and input owner before the normal chat send. */
export function admitSetupGuideResume(resume, guide, receipts, flags, input, respond) {
  const current = setupGuideResume(guide, receipts, flags);
  if (!current || resume?.guide_id !== current.guide_id || resume.guide_version !== current.guide_version ||
      resume.text !== current.text || input?.key !== inputKey(guide)) return false;
  respond('chat', { text: current.text }, { preserveInput: true,
    expectedGuide: { id: current.guide_id, version: current.guide_version } });
  return true;
}

/** An earlier rollback cannot describe a different installed source or candidate. */
function currentRestoredUpdate(deployment, build, guide) {
  const update = deployment.update, hash = /^[a-f0-9]{64}$/;
  return update?.status === 'rolled-back' &&
    /^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$/.test(update.update_id || '') &&
    typeof deployment.build_id === 'string' && !!deployment.build_id && update.from_build_id === deployment.build_id &&
    hash.test(update.previous_package_sha256 || '') && update.previous_package_sha256 === deployment.package_sha256 &&
    hash.test(update.previous_source_digest || '') &&
    (deployment.source_digest == null || deployment.source_digest === update.previous_source_digest) &&
    implementationIsCurrent(build, guide) && build.source_verified === true &&
    update.to_build_id === build.build_id && update.to_build_id !== update.from_build_id &&
    update.guide_version === build.guide_version &&
    hash.test(update.source_digest || '') && update.source_digest === build.source_digest &&
    hash.test(update.package_sha256 || '');
}

/** Explain current host receipts without rewriting the guide's historical claims. */
export function setupProgress(guide, { build, trial, deployment, remoteDeployment } = {}, flags = {}) {
  const { pendingChanges = false, designBusy = false, agentTeamAvailable = true } = flags;
  if (!guide?.id || !(guide.proposal || guide.blueprint) || pendingChanges || designBusy) return null;
  const buildKnown = buildObservationKnown(flags);
  const admissionPending = flags.buildAdmissionPending === true;
  const buildStatus = admissionPending ? {
    title: 'Build request in progress',
    detail: 'Your build request is still in progress. Waiting for its saved build status.',
    actions: [action('build', 'Follow build request')],
  } : { title: flags.buildObservation === 'loading' ? 'Checking your saved build' : 'Build status is unavailable',
    detail: flags.buildObservation === 'loading'
      ? 'Forge is checking for saved application work before starting a build. You can keep editing your builder note while this is checked.'
      : 'Forge could not check the latest build status. Open the builder to refresh it; your note and saved source are kept.',
    actions: [action('build', 'Review build status')] };
  if (!buildKnown && build?.guide_id === guide.id && build.build_id)
    buildStatus.detail += ' Saved build findings remain available.';
  const withBuildObservation = (progress) => buildKnown && !admissionPending ? progress : { ...progress,
    detail: `${progress.detail} ${buildStatus.detail}`,
    actions: [...progress.actions.filter(item => item.target !== 'build'), ...buildStatus.actions] };
  const matchingTrial = applicationTrialMatches(trial, build, guide);
  const ownedActiveTrial = trial?.guide_id === guide.id && !!trial.rehearsal_id && rehearsalActive(trial);
  const currentTrial = trial?.mode === 'application' ? matchingTrial : rehearsalCurrent(trial, guide);
  const trialBuildUnknown = !buildKnown && trial?.mode === 'application';
  const waiting = trial?.status === 'waiting_input', ending = ['ending', 'canceling'].includes(trial?.status);
  const settling = trial?.status === 'running' && trial?.session_status === 'settling';
  const canContinue = rehearsalCanContinue(trial, build, guide);
  const endToRepair = applicationTrialEndToRepair(trial, build, guide);
  const activeBuild = buildKnown && ownedRunningBuild(guide, build)
    ? activeBuildProgress(build, !implementationIsCurrent(build, guide)) : null;
  const cleanup = activeBuild && (build.cleanup_in_progress === true || build.stop_requested === true ||
    build.cleanup_pending === true && ['completed', 'failed', 'interrupted'].includes(build.status)) ? activeBuild : null;
  const retainedBuild = cleanup || (activeBuild && !implementationIsCurrent(build, guide) ? activeBuild : null);
  const withCleanup = (progress) => !retainedBuild ? progress : { ...progress,
    detail: `${progress.detail} ${retainedBuild.detail}`,
    actions: [...progress.actions.filter(item => item.target !== 'build'), ...retainedBuild.actions],
    ...(retainedBuild.attention ? { attention: true } : {}) };
  const followAction = trialAction(endToRepair ? 'Review trial problem' : canContinue ? 'Continue trial' : 'Follow trial');
  const withActiveTrial = (progress) => withBuildObservation(!ownedActiveTrial ? progress : {
    ...progress,
    detail: `${progress.detail} ${ending ? trial.status === 'canceling' ? 'A saved trial is stopping.' : 'A saved trial is ending.'
      : endToRepair ? 'An application callback failed or was unavailable. End the saved trial; after cleanup is confirmed, choose Fix this trial to send its findings to the builder.'
      : trialBuildUnknown ? waiting ? 'A saved application trial is waiting for input.'
        : settling ? 'A saved application trial is settling its remaining native work.' : 'A saved application trial is also running.'
      : waiting ? currentTrial ? 'A trial of this proposal is waiting for a follow-up.'
        : 'An earlier trial is still open and waiting for input. It cannot continue with this proposal.'
      : settling ? currentTrial ? 'A trial of this proposal is settling its remaining native work.' : 'An earlier trial is settling its remaining native work.'
      : currentTrial ? 'A trial of this proposal is also running.' : 'An earlier trial is also running.'}`,
    actions: [...progress.actions.filter(item => item.target !== 'trial'), followAction],
  });
  const localInstallation = deployment?.guide_id === guide.id && typeof deployment.deployment_id === 'string' && !!deployment.deployment_id;
  const remoteProgress = ownedRemoteInstallation(remoteDeployment, guide) || !localInstallation
    ? remoteInstallationProgress(remoteDeployment, guide, flags) : null;
  if (remoteProgress) return withActiveTrial(withCleanup(remoteProgress));
  if (!deploymentObservationKnown(flags)) {
    const installationStatus = { title: flags.deploymentObservation === 'loading'
      ? 'Checking your installation status' : 'Installation status is unavailable',
      detail: flags.deploymentObservation === 'loading'
        ? 'Forge is checking your installation before treating its saved status as current. Your notes and setup drafts remain available.'
        : 'Forge could not check your current installation status. Open its status controls to check again; your notes and setup drafts are kept.',
      actions: [action('deployment', 'Review installation status')] };
    if (deployment?.guide_id === guide.id && deployment.deployment_id)
      installationStatus.detail += ' Saved installation findings remain available.';
    return withActiveTrial(withCleanup(installationStatus));
  }
  if (deployment?.guide_id === guide.id && deployment.deployment_id) {
    const nativeTeam = deploymentSourceKind(deployment) === 'native-team';
    const newer = buildKnown && implementationIsCurrent(build, guide) && build.build_id !== deployment.build_id;
    const newerAccepted = newer && deploymentCanPrepare(build, guide) &&
      implementationAcceptance(build, guide)?.passed && build.source_verified === true;
    const restored = buildKnown && currentRestoredUpdate(deployment, build, guide);
    const consoleUrl = deploymentConsole(deployment);
    if (consoleUrl) {
      const installedProposal = Number.isSafeInteger(deployment.guide_version)
        ? `Installed proposal ${deployment.guide_version}; current proposal ${guide.version}. ` : '';
      return withActiveTrial(withCleanup({ title: nativeTeam
        ? restored ? 'Your previous installed agent team is running' : 'Your installed agent team is running'
        : restored ? 'Your previous installed custom application is running' : 'Your installed custom application is running',
        detail: installedProposal + (restored ? 'The previous installed source was restored. Review the saved update and recovery findings before another attempt. The newer candidate is separate from this running source.'
          : newer && implementationIsRunning(build)
          ? cleanup ? 'Your installed application stays unchanged.' : 'The builder is preparing and checking an improvement. Your installed application stays unchanged.'
          : newerAccepted ? nativeTeam ? 'A checked custom application is available separately. This installed agent team stays unchanged.'
            : 'A newer application passed independent offline checks. Review it before updating the installed version.'
          : newer ? 'A newer application candidate still needs work. Review its saved findings; your installed application stays unchanged.'
          : nativeTeam ? 'Use the named conversation owner in the Console for your first job, then check its actual reply and saved results.'
            : 'Open the installed application below to try your first job. Its live connections and application workflows still need their own checks.'),
        actions: [action('deployment', restored ? 'Review recovery findings' : nativeTeam ? 'Use your agent team' : 'Use your application'),
          { target: 'console', label: nativeTeam ? 'Open agent team Console' : 'Administration Console', url: consoleUrl }] }));
    }
    if (['failed', 'interrupted', 'stopped'].includes(deployment.status) && activeBuild) {
      return withActiveTrial({ ...activeBuild,
        detail: `${activeBuild.detail} Your earlier installation and its saved findings remain available.`,
        actions: [...activeBuild.actions, action('deployment', deployment.status === 'stopped' ? 'Manage application' : 'Review installation')] });
    }
    if (['queued', 'installing', 'checking', 'starting', 'stopping', 'running'].includes(deployment.status)) {
      return withActiveTrial(withCleanup({ title: deployment.status === 'running' ? 'Your running application needs a health check' : 'Your installation is in progress',
        detail: 'Follow the actual installation status and checks below.', actions: [action('deployment', 'View installation')] }));
    }
    if (['failed', 'interrupted'].includes(deployment.status)) {
      return withActiveTrial({ title: 'Your installation needs attention', detail: 'The saved installation explains what stopped and how to recover.',
        actions: [action('deployment', 'Review installation')] });
    }
    if (deployment.status === 'stopped') {
      const accepted = implementationAcceptance(build, guide);
      const canTry = buildKnown && !nativeTeam && guide.readiness?.context_ready === true && deploymentCanPrepare(build, guide) &&
        accepted?.passed && build.source_verified === true;
      return withActiveTrial({ title: nativeTeam ? 'Your installed agent team is stopped' : 'Your installed application is stopped',
        detail: restored ? 'The previous installed source was restored and is now stopped. Review the saved update and recovery findings before another attempt. The newer candidate remains separate from this installed source.'
          : newerAccepted ? 'Manage the stopped installed source here. A newer application passed independent offline checks; its trial uses that newer candidate and does not update this installation.'
          : newer ? 'Manage the stopped installed source here. A newer candidate still needs work; its saved findings are separate from this installation.'
          : nativeTeam ? 'Start the installed team to resume its agents and records. A changed plan needs a separate installation.'
            : 'Use its operation controls to start the installed source again or review an update.',
        actions: [action('deployment', restored ? 'Review recovery findings' : nativeTeam ? 'Manage agent team' : 'Manage application'),
          ...(canTry ? [trialAction(newer ? 'Try the newer application' : 'Try the application', 'application')] : [])] });
    }
  }
  if (ownedActiveTrial) {
    return withBuildObservation(withCleanup({ title: trial.status === 'canceling' ? 'Your trial is stopping'
      : ending ? 'Your trial is ending'
      : endToRepair ? 'Your trial needs an application fix'
      : trialBuildUnknown ? waiting ? 'Your saved trial is waiting for input'
        : settling ? 'Your saved trial is settling' : 'Your saved application trial is running'
      : waiting ? currentTrial ? 'Your trial is waiting for a follow-up' : 'An earlier trial is still open'
      : settling ? currentTrial ? 'Your trial is settling' : 'An earlier trial is settling'
      : !currentTrial ? 'An earlier trial is still running'
      : trial.mode === 'application' ? 'Your application trial is running' : 'Your design trial is running',
      detail: ending ? 'The trial is ending. Follow its saved activity while its workers stop.'
        : endToRepair ? 'An application callback failed or was unavailable. End this trial; once cleanup is confirmed, choose Fix this trial to send the saved findings to the builder.'
        : trialBuildUnknown ? canContinue ? 'Continue the same isolated conversation and sample state, or end the saved trial. Live connections are not tested here.'
          : 'Follow or end this saved trial before starting another. Its relationship to the latest application build has not been confirmed.'
        : !currentTrial ? 'Follow or end its saved trial before starting another. It belongs to an earlier proposal or application source and cannot continue with the current design.'
        : waiting ? canContinue ? 'Continue the same isolated conversation and sample state, or end the trial. Live connections are not tested here.'
          : 'This trial cannot accept a follow-up now. Review its activity or end it before starting another trial.'
        : settling ? 'Additional native work is still settling in the same trial. Follow its actual activity; a follow-up becomes available only when it is ready for input.'
        : trial.mode === 'application' ? 'Follow the actual agents and isolated callback results. This does not test your custom UI or live connections.'
        : 'Follow how the proposed agents collaborate. Application code and live connections are not part of this design trial.',
      actions: [followAction] }));
  }
  if (admissionPending || !buildKnown) return buildStatus;
  if (cleanup) return cleanup;
  if (activeBuild && !implementationIsCurrent(build, guide)) return withActiveTrial(activeBuild);
  if (!implementationIsCurrent(build, guide)) {
    const packageReason = packagePreparationReason(guide);
    if (packageReason) return { kind: 'package-pending', title: 'Your design is saved; package preparation is pending',
      detail: packageReason, actions: [], attention: true };
    if (setupNeedsApplicationBuild(guide, build, flags)) return { title: 'Your application needs to be built',
      detail: 'Your current plan includes application work that the agent team package does not implement. Open the builder to create and check the application from this proposal.',
      actions: [action('build', 'Build application')] };
    // A Forge that offers no agent team package leads to the builder instead.
    if (agentTeamAvailable && nativeTeamCanInstall(guide)) return { title: 'Your agent team is ready to install',
      detail: 'Check the selected destination, then set up private credentials and start the Console, agents and tools. No custom application build is needed.',
      actions: [installAction('Install agent team', 'native-team'), action('package', 'Download package'),
        action('build', 'Optional custom application')] };
    if (agentTeamAvailable && nativeTeamPackagePlan(guide)) return { title: 'Your agent team package is available',
      detail: 'Get the Console and team from your current plan. Use the builder when your outcomes need custom screens or application code.',
      actions: [action('package', 'Get your agent team'), action('build', 'Optional custom application')] };
    if (guide.readiness?.context_ready === true && (guide.readiness?.decision_blockers?.length || 0) > 0) return { kind: 'discovery', title: 'Resolve your open setup decisions',
      detail: 'Continue with the guide before exporting this team or starting a custom application.', actions: [] };
    if (guide.readiness?.context_ready === true) return { title: 'You can start building',
        detail: 'The builder turns your proposal into an application. You can continue refining it together.',
        actions: [action('build', 'Open builder')] };
    return pausedSetupDiscovery(guide)
      ? { kind: 'discovery', title: 'Continue shaping your setup',
        detail: "Resume the guide's questions from your saved proposal, earlier answers and manual edits.",
        actions: [] } : null;
  }
  if (implementationIsRunning(build)) {
    return activeBuild;
  }
  if (matchingTrial && applicationTrialRepair(trial, build, guide)) {
    return { title: 'Your application trial needs attention',
      detail: 'Review the saved trial problem and send its observations to the builder. The trial is separate from installation and live connection checks.',
      actions: [trialAction('Review trial', 'application')] };
  }
  const accepted = implementationAcceptance(build, guide);
  if (deploymentCanPrepare(build, guide) && accepted?.passed && build.source_verified === true) {
    if (matchingTrial && trial.status === 'completed') {
      return { title: 'Your application trial has finished',
        detail: 'Review what the sample actually did. A completed agent turn does not establish that every requested behavior worked.',
        actions: [trialAction('Review trial', 'application'), installAction('Set up and run', 'application')] };
    }
    return { title: 'Your application passed independent offline checks',
      detail: 'Try it with sample data, then configure its destination and private settings.',
      actions: [trialAction('Try the application', 'application'), installAction('Set up and run', 'application')] };
  }
  if (['failed', 'interrupted', 'completed'].includes(build.status)) {
    return { title: 'Your application still needs work', detail: 'Review the saved findings and continue the builder from the checked source.',
      actions: [action('build', 'Review and repair')] };
  }
  return null;
}
