# Authorization operations UX and DX

## Status

Internal implementation guidance. This describes an operator experience, not a
claim that every authorization surface is already enforced.

## Context

Meerkat needs one understandable way to inspect and configure who may perform
which operations, while keeping authentication, authorization policy, and each
feature's enforcement owner distinct. MobKit already has a console Access
workflow for its console ABAC configuration. Treating that screen as the
universal policy editor would conflate MobKit's agent visibility rules with
Meerkat's operation authorization contracts.

The administration experience should reuse familiar Meerkat configuration
entrypoints and surface-owned APIs. It should make effective decisions
inspectable without implementing a second policy evaluator in the browser.

## Agreed operating contract

The default governed profile uses local authorization on the trusted host.
Checks must be inexpensive enough to preserve ordinary operation latency.
External witnesses, authenticated time, and human-gated recovery belong to an
optional higher-assurance profile. They are not prerequisites for ordinary
tool calls, source reads, or communications.

A refusal belongs to the attempted operation. Return typed feedback through the
existing model/tool loop, retain the admitted controller, and let the agent
choose another operation in the same run and session. Distinguish a policy
denial from unavailable checks or failed audit recording; none should be
presented as a successful effect. Unsupported confinement also refuses the
attempt locally before launching the target.

Authorize concrete boundaries: context admission, source reads, tool effects,
communications, persistence, and publication. Audit can record identities,
resource references, and operation outcomes. It cannot prove that an LLM's
answer contains no semantic information learned from an earlier input. Do not
offer semantic taint tracking or automatic declassification as an enforcement
guarantee.

The completion criterion covers every reachable protected operation across
the supported surfaces. A small end-to-end checkpoint is useful for integration
and testing, but does not define a permanently narrow product profile.

## Operator flow

1. **Overview establishes scope.** Show the connected runtime or realm, the
   authenticated subject, the active enforcement state, and whether edits
   persist to a file or only to the current process. Put owner and revision
   diagnostics behind a disclosure.
2. **Subjects and groups explain identity.** Show authenticated identifiers as
   identifiers, not presumed email addresses. Make group membership searchable
   and allow an operator to inspect a subject's effective access directly.
3. **Rules read as a sentence.** Present each rule as “Who can do what on which
   resources?” Use searchable selectors for subjects, groups, actions, and
   known resource identities. Explain the meaning of an empty selector beside
   the editor. Keep low-level labels and roles available as advanced fields.
4. **Preview asks about one operation.** Let the operator select a subject,
   action, and resource. Ask the runtime owner for the decision and explanation;
   display the effective scope, decision, and whether an administrator grant
   determined the result. Link to matching rules only when the owner returns
   stable rule identifiers. Do not duplicate policy evaluation in the UI.
5. **Save shows the change.** Summarize the draft against the current revision
   before saving. On a revision conflict, keep the draft and show the current
   value beside it; the operator can review and reapply the intended changes.
6. **Denials stay with the operation.** Show a concise refusal beside the
   attempted action. Preserve distinct messages for permission denial,
   unavailable authorization checks, and audit update failure. Offer “Inspect
   access” only when the caller has an authorized inspection surface.

## Ownership boundaries

- Keep MobKit's `config/access.toml` and its Rust, Python, and TypeScript
  builders as the configuration path for the existing console ABAC feature.
- Keep realm-scoped raw and effective configuration semantics in the Meerkat
  configuration owner.
- Add operation-specific administration to an owner-backed Meerkat surface
  only when that owner can return the effective decision and its explanation.
  A console presentation setting must never imply runtime permission.
- Keep tool, source, publication, and communication permissions in their
  respective feature-owned adapters over the shared authorization contract.
  Do not show a permission as configurable while its enforcement adapter is
  absent.

## First UI slice

The lowest-risk MobKit improvement is to make existing Rules and Preview
workflows easier to use: searchable rule fields, an “Inspect access” action
from a subject, and a readable save diff. Native operation permissions should
appear in a separately named section after the runtime owner exposes the
necessary administration and decision-explanation APIs.

The key implementation references from the console UX review are
`console/src/panels/AccessPanel.tsx` for Access tabs and preview contracts,
`console/src/ConsoleApp.tsx` for owner/revision and conflict handling, and
`packages/console-core/src/operation-feedback.ts` for distinct operation
feedback states. The current preview result contains `allowed`, `reason`,
`groups`, and `is_admin`; detailed matched-rule explanations require an
owner-provided contract extension.

## Host confinement configuration

The native shell owner now accepts `ShellConfig::confinement`. Install
`ShellConfinement::Required` in trusted host configuration for confined
execution. Foreground calls, background jobs, and recovered monitors use the
manager's compiled requirement; recovered job metadata does not select the
mode. Model-proposed arguments cannot weaken it. A shared tool and manager
must have the same confinement value, and changing that value requires a new
owner configuration.

This example grants workspace reads and writes, denies IP networking, and
grants no Unix socket connections. Canonicalize an existing workspace before
creating path grants, including macOS `/tmp` and `/var` aliases.

```rust
use meerkat_core::confinement::{
    ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess,
    PathAccess, PlatformBaseline,
};
use meerkat_tools::builtin::shell::{ShellConfig, ShellConfinement};

fn confined_shell_config(
    workspace: &std::path::Path,
) -> Result<ShellConfig, Box<dyn std::error::Error>> {
    let work = std::fs::canonicalize(workspace)?;
    let requirement: ExecutionConfinement = ConfinementSpec {
        baseline: PlatformBaseline::CommandRuntimeV1,
        read: FilesystemAccess::Paths(vec![PathAccess::Subtree(work.clone())]),
        write: FilesystemAccess::Paths(vec![PathAccess::Subtree(work.clone())]),
        deny_read: vec![],
        deny_write: vec![],
        network: IpNetworkAccess::Denied,
        unix_connect: vec![],
        require_descendant_termination: false,
    }
    .try_into()?;
    Ok(ShellConfig {
        enabled: true,
        shell: "sh".into(),
        shell_path: Some("/bin/sh".into()),
        project_root: work,
        env_vars: [("PATH".into(), "/usr/bin:/bin".into())].into(),
        confinement: ShellConfinement::Required { requirement },
        ..ShellConfig::default()
    })
}
```

`CommandRuntimeV1` also grants its enumerated system runtime resources, such
as loaders, libraries, and null/random devices. It does not grant ambient home,
scratch, credential, network, or service IPC access. Explicit denials take
precedence over grants.

With this configuration, omitting `working_dir` uses the canonical project root.
Relative directories resolve from that root; an explicit directory is
canonicalized and must stay within the root while `restrict_to_project` is
true. Filesystem confinement applies to the process's I/O independently of
this directory check.

Required launches inherit no parent environment. The host starts the launch
map with `PWD` set to the resolved directory, then overlays `env_vars`.
An explicit `PWD` therefore changes that variable, not the actual directory.
Monitors add their submission key and any resume checkpoint last. Set `PATH`
and each application variable explicitly; the host does not inherit `HOME`,
proxy settings, or credentials. Loader and shell startup injection variables
are refused rather than silently removed. The executed shell may define its
own variables after launch.

The current backend is macOS Seatbelt. Required confinement on other platforms
returns `ConfinementRefusal::UnsupportedRequirement` before the target runs.
The macOS backend also refuses exact IP endpoint grants and requirements to
prove termination of descendants that leave the process group. Setting
`require_descendant_termination: false` in this example makes that limit
explicit. Unsupported requirements never fall back to trusted execution.

`ShellConfinement::TrustedHost` is the compatibility default for old
configuration that omits `confinement`. It retains ambient environment
inheritance with the configured overlay and does not provide this confinement
guarantee. A malformed `Required` value is a configuration error, not a request
for compatibility mode.

Keep launch refusals beside the attempted operation. The native owner returns
bounded reasons such as `InvalidRequirement`, `InvalidLaunch`,
`UnsupportedRequirement`, `BackendUnavailable`, and `PreparationFailed`
through existing tool feedback. A syscall denied after launch is instead
reported through shell output and exit status; inspect those values before
claiming command success. Either outcome leaves later allowed operations
available in the same session.

When durable `ProcessCustody` is bound, the target waits behind the existing
spawn gate until its PID identity is recorded. Release the custody record only
after the process group's exit is observed. An accepted kill or cancellation
request is an execution fence, not proof of exit. The regression for custody
retention after an accepted kill remains part of checkpoint acceptance.

Command-hook integration is pending. Do not display hooks as confined until
their owner uses the same sealed launch boundary. Show the effective native
mode only on surfaces backed by this owner; CLI, API, SDK, and console
configuration coverage remains integration work.

Validate inexpensive operation checks with a quiet benchmark window after the
functional slice passes. Report authorization overhead separately from model
latency and compilation time. A fast test body is not benchmark acceptance.
