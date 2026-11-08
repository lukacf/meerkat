/** Host-owned Services v5. Private payloads must not enter model messages or logs. */
import { ServiceTransport, setupLaunch, type ServiceSetupBinding } from "./service-transport.js";
export { ServiceSetupError, ServiceSetupTimeout, ServiceSetupEnded, ServiceSetupLaunchError,
  ServiceSetupEvent } from "./service-transport.js";
export type { ServiceSetupBinding, ServiceSetupAnswer, ServiceConnectorTarget,
  ServiceConnectorOperation, ServiceSetupClose } from "./service-transport.js";

/** One retained protocol5 process; no automatic retry or protocol upgrade. */
export class ServiceSetupClient extends ServiceTransport {
  constructor(executable: string, binding: ServiceSetupBinding, environment?: Readonly<Record<string, string>>) {
    super(setupLaunch(executable, binding, environment));
  }
}
