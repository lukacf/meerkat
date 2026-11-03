// Synthetic human-view data, never a live authorization or provider receipt.
export function control(kind, reference = kind) {
  return { offer: `${kind}:${reference}:offer-1`, kind, target: { owner: "fixture-owner", reference, revision: "attempt-1" } };
}

export function fixture(revision = 1) {
  const service = {
    kind: "service", id: "docs-access", label: "Read support documentation",
    status: { tone: "ready", text: "Access ready for this task" }, account: "Acme support library",
    usedBy: "Work Assistant / support setup task", operations: ["Search", "Read documents"],
    resources: ["Support collection"], lifetime: "Until this task finishes",
    standingAuthority: "Existing Support knowledge policy",
    controls: [control("revoke_grant", "docs-grant")],
  };
  const frontend = {
    kind: "frontend", id: "support-slack", packageId: "slack-package",
    label: "Work Assistant in Slack", status: { tone: "pending", text: "Waiting for administrator installation" },
    endpoint: "Acme / #support-private", audience: "Approved Support group",
    target: "Work Assistant", conversation: "Isolated per admitted thread and audience",
    output: "Replies in the same thread; private agent transcript excluded",
    serviceAccess: "Support knowledge base under the existing support policy",
    lifetime: "Until paused or removed", controls: [control("remove_frontend", "slack-frontend")],
  };
  return {
    scope: "fixture-viewer", revision, observedAt: "2026-10-01 09:30 UTC",
    announcement: "Support documentation access is ready. The Slack frontend is waiting for administrator installation.",
    tasks: [{
      id: "support-task", title: "Answer support questions in Slack", application: "Acme / Work Assistant",
      requester: "Sam (authenticated fixture person)", reason: "Let our Support group ask questions using our docs.",
      phase: { tone: "pending", text: "One role ready; frontend setup pending" },
      nextAction: "An administrator needs to install the Slack app.", roles: [service, frontend],
      userActions: [{
        id: "slack-auth", integration: "Slack frontend", kind: "browser_authorization", state: "action_required",
        instructions: "Review the requesting integration and destination, then continue in your external browser.",
        expiresAt: "2026-10-01 10:00 UTC", authorizationOrigin: "https://slack.example.test", requestedAccountLabel: "Work account",
        controls: [control("open_authorization", "slack-auth")],
      }], controls: [control("stop_setup", "support-setup"), control("cancel_task", "support-task")],
      diagnostics: "Synthetic demonstration. No Slack account, installation or delivery has been verified.",
    }],
    connections: [{
      id: "docs-connection", packageId: "docs-package", service: "Support documentation",
      account: "Acme support library", credentialScope: "Library read scope",
      status: { tone: "ready", text: "Account connected" }, grants: [service],
      pendingTaskIds: ["support-task"], controls: [control("change_account", "docs-account"), control("disconnect", "docs-account")],
      diagnostics: "Connection health and grants are separate owner observations.",
    }],
    frontends: [frontend],
    packages: [{
      id: "slack-package", label: "Slack integration", version: "fixture",
      status: { tone: "pending", text: "Activation proposed" },
      exports: [{ kind: "frontend", id: "slack-agent-frontend", label: frontend.label,
        status: frontend.status, instanceIds: [frontend.id] }],
      components: [{ label: "Transport", status: { tone: "pending", text: "Not deployed" } }],
    }, {
      id: "docs-package", label: "Documentation integration", version: "fixture",
      status: { tone: "ready", text: "Package active" },
      exports: [{ kind: "service", id: "documentation-service", label: "Documentation connection",
        status: service.status, instanceIds: ["docs-connection"] }],
      components: [],
    }],
  };
}
