import { ServiceCollectionClient, ServiceSetupEvent, ServiceSetupLaunchError } from "../src/service-collection.js";
import type { ServiceCollectionLauncher, ServiceCollectionReport, ServiceSetupBinding } from "../src/service-collection.js";
import { ServiceProbeClient } from "../src/service-probe.js";
import type { ServiceProbeLauncher } from "../src/service-probe.js";
declare const launcher: ServiceCollectionLauncher;
declare const binding: ServiceSetupBinding;
declare const event: ServiceSetupEvent;
const client = new ServiceCollectionClient(launcher, binding);
const id: string = client.collect({job_id: "observed-job", source_id: "observed-source"});
client.cancel(id);
const report: Readonly<ServiceCollectionReport> | undefined = client.collectionReport(event);
if (report) { const coverage: "unqualified_credential_continuity" = report.reusable_source_coverage; void coverage; }
declare const failure: ServiceSetupLaunchError;
void failure.owner.close();
// @ts-expect-error a stock executable string cannot create a trusted collection host
new ServiceCollectionClient("/native/ordinary", binding);
// @ts-expect-error v6 is not a v7 launcher
new ServiceCollectionClient({...launcher, protocol_version: 6}, binding);
// @ts-expect-error browser programs and operations are not a wire selector
client.collect({job_id:"job",source_id:"source",program:{},operations:["write"]});
// @ts-expect-error resources cannot be substituted on the wire
client.collect({job_id:"job",source_id:"source",resource:{}});
// @ts-expect-error observations cannot qualify credential continuity
if (report) report.reusable_source_coverage = "qualified";
// @ts-expect-error nested native observations are immutable
if (report) report.requirements[0]!.calls[0]!.native.native_close_joined = true;
// @ts-expect-error no SDK authority constructor or readiness certificate exists
client.ready();
declare const oldLauncher: ServiceProbeLauncher;
// @ts-expect-error existing v6 constructor does not gain collection admission
new ServiceProbeClient(oldLauncher,binding).collect({job_id:"job",source_id:"source"});

const nextId: string = client.openNextService({service_id: "observed-next-service"});
void nextId;
const serviceIds: readonly string[] | undefined = client.state?.service_ids;
void serviceIds;
// @ts-expect-error a next-service request carries no native authority
client.openNextService({service_id:"next",caller:{},receipt:{}});
// @ts-expect-error a session cannot replace the preselected service identifier
client.openNextService({session_id:"new"});
// @ts-expect-error service selection remains immutable
client.state!.service_ids!.push("injected");
