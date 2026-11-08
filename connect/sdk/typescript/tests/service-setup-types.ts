import { ServiceSetupClient } from "../src/service-setup.js";
import type { ServiceSetupBinding, ServiceSetupAnswer, ServiceSetupClose } from "../src/service-setup.js";

declare const executable: string;
declare const binding: ServiceSetupBinding;
const owner = new ServiceSetupClient(executable, binding);
const answer: ServiceSetupAnswer = { kind: "service_secret", job_id: "job", service: "service", slot: "slot", value: "synthetic" };
owner.answer("p:secret", answer);
const close: Promise<ServiceSetupClose> = owner.close();
void close;
// @ts-expect-error browser/device notices have no fabricated completion answer
owner.answer("p:browser", { kind: "provider_browser", callback: "private" });
// @ts-expect-error exact secret selectors are required
owner.answer("p:secret", { kind: "service_secret", value: "synthetic" });
// @ts-expect-error readiness is not a client operation
owner.ready();
// @ts-expect-error caller-controlled connector profiles are not v5 initialization
new ServiceSetupClient(executable, { ...binding, connector_auth: { profiles: [] } });
