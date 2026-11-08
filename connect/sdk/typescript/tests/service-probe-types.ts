import { ServiceProbeClient, ServiceSetupLaunchError, ServiceSetupEvent } from "../src/service-probe.js";
import { ServiceSetupClient } from "../src/service-setup.js";
import type { ServiceProbeLauncher, ServiceProbeReport, ServiceSetupBinding, ServiceCommissioning } from "../src/service-probe.js";

declare const launcher: ServiceProbeLauncher;
declare const binding: ServiceSetupBinding;
declare const event: ServiceSetupEvent;
const client = new ServiceProbeClient(launcher, binding);
const id: string = client.probe({ job_id: "observed-job", profile_id: "observed-profile" });
client.cancel(id);
const report: Readonly<ServiceProbeReport> | undefined = client.probeReport(event);
const commissioning: Readonly<ServiceCommissioning> | undefined = client.commissioning(event);
const current: boolean = client.viewerCurrentAtObservation(event);
void [report, commissioning, current, client.close()];
// Retained process owner remains usable for close even if launch interrupted.
declare const launchFailure: ServiceSetupLaunchError;
const retainedV5Shape: ServiceSetupClient = launchFailure.owner;
void retainedV5Shape.close();
// @ts-expect-error ordinary executable selection cannot imply a trusted v6 launcher
new ServiceProbeClient("/path/to/stock/meerkat-connect", binding);
// @ts-expect-error protocol5 cannot select protocol6
new ServiceProbeClient({ ...launcher, protocol_version: 5 }, binding);
// @ts-expect-error trusted resource profiles never arrive through probe parameters
client.probe({ job_id: "job", profile_id: "profile", resource: { repository: "other" } });
// @ts-expect-error native operation arguments are not browser-controlled probe input
client.probe({ job_id: "job", profile_id: "profile", arguments: {} });
// @ts-expect-error reports are immutable historical observations
if (report) report.status = "capability_verified";
// @ts-expect-error commissioning can never attest resource capability
if (commissioning) commissioning.capability_verified = true;
// @ts-expect-error the SDK does not mint a reusable Build readiness permit
client.ready();
// @ts-expect-error v5 does not acquire the new probe operation
new ServiceSetupClient("/absolute/native", binding).probe({ job_id: "job", profile_id: "profile" });
