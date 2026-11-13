import type { InstructionTarget } from "../src/index.js";

// A member target may carry the fork scope; the other kinds have none.
const family: InstructionTarget = {
  kind: "member",
  agent_identity: "worker-1",
  include_forks: true,
};
const alone: InstructionTarget = { kind: "member", agent_identity: "worker-1" };
const explicit: InstructionTarget = {
  kind: "member",
  agent_identity: "worker-1",
  include_forks: false,
};
const notABoolean: InstructionTarget = {
  kind: "member",
  agent_identity: "worker-1",
  // @ts-expect-error The fork scope is a boolean.
  include_forks: "yes",
};
const everyMember: InstructionTarget = {
  kind: "mob",
  // @ts-expect-error Only a member target has a fork scope.
  include_forks: true,
};
const oneProfile: InstructionTarget = {
  kind: "profile",
  profile: "worker",
  // @ts-expect-error Only a member target has a fork scope.
  include_forks: true,
};

void [family, alone, explicit, notABoolean, everyMember, oneProfile];
