use crate::{
    GheRepository, HistoricalObservation, HistoricalStore, ParsedHistory, ProgramLimits,
    ProgramSpec, catalog::Engine,
};
use forge_ghe_collections::{ProjectedBlock, ReadCall, projected_github_payload};
use meerkat_connect::{
    service_collection::{
        CollectionCall, CollectionCursor, CollectionProgram, CollectionStep, CollectionTarget,
        CommittedCollectionResult, ReadObservation, RequiredCollectionOperation,
    },
    service_probe::ResourceAssessment,
};
use meerkat_core::{ContentBlock, McpServerConfig, mcp_config::McpTransportConfig};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path, sync::Arc};

#[derive(Debug, PartialEq, Eq)]
pub enum AdapterError {
    InvalidSelection,
    NativeContractRefused,
    HistoryUnavailable,
    HistoryFull,
}
const KNOWN_OPERATIONS: [&str; 8] = [
    "actions_get",
    "actions_list",
    "get_commit",
    "issue_read",
    "list_commits",
    "list_issues",
    "list_pull_requests",
    "pull_request_read",
];

struct Binding {
    source_id: String,
    requirement_id: String,
    resource: Value,
    operations: BTreeSet<String>,
    installed: McpServerConfig,
}
impl Binding {
    fn matches(&self, target: &CollectionTarget<'_>) -> bool {
        self.source_id == target.source_id()
            && self.requirement_id == target.required_operation_id()
            && &self.resource == target.resource()
            && &self.operations == target.raw_operation_ceiling()
            && &self.installed == target.installed_connection()
    }
}

/// Exact selected configuration is supplied by protected application composition.
/// Equality preserves its account/endpoint expectations; it does not verify the
/// account, executable bytes, credential continuity or caller. Those are native
/// owners' responsibilities, and cannot be supplied by a JSON success flag.
pub struct BoundGheProgram {
    binding: Arc<Binding>,
    repository: GheRepository,
    spec: ProgramSpec,
    limits: ProgramLimits,
    history: Arc<HistoricalStore>,
}
impl BoundGheProgram {
    pub fn new(
        source_id: String,
        requirement_id: String,
        installed: McpServerConfig,
        repository: GheRepository,
        spec: ProgramSpec,
        limits: ProgramLimits,
        history: Arc<HistoricalStore>,
    ) -> Result<Self, AdapterError> {
        let name = |s: &str| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control);
        if !name(&source_id) || !name(&requirement_id) {
            return Err(AdapterError::InvalidSelection);
        }
        // Build once to reject invalid selectors before any native admission.
        spec.build(&repository, limits)
            .map_err(|_| AdapterError::InvalidSelection)?;
        let operations: BTreeSet<_> = spec.operations().into_iter().collect();
        if !installation_matches(&installed, &repository.host, &operations) {
            return Err(AdapterError::InvalidSelection);
        }
        let resource = spec.resource(&repository, limits);
        if resource.to_string().len() > 4096 {
            return Err(AdapterError::InvalidSelection);
        }
        Ok(Self {
            binding: Arc::new(Binding {
                source_id,
                requirement_id,
                resource,
                operations,
                installed,
            }),
            repository,
            spec,
            limits,
            history,
        })
    }
    /// The caller composes this exact row into its original source. Neither the
    /// source ID nor the raw operation ceiling is merged or derived from prose.
    pub fn requirement(self: &Arc<Self>) -> Result<RequiredCollectionOperation, AdapterError> {
        let program: Arc<dyn CollectionProgram> = self.clone();
        RequiredCollectionOperation::new(
            self.binding.requirement_id.clone(),
            self.binding.resource.clone(),
            self.binding.operations.iter().cloned().collect(),
            program,
        )
        .map_err(|_| AdapterError::NativeContractRefused)
    }
    pub fn source_id(&self) -> &str {
        &self.binding.source_id
    }
}

fn installation_matches(
    config: &McpServerConfig,
    host: &str,
    operations: &BTreeSet<String>,
) -> bool {
    let McpTransportConfig::Stdio(stdio) = &config.transport else {
        return false;
    };
    let Some(access) = &config.access else {
        return false;
    };
    // No false declaration that a PAT-backed source has no selected account.
    if config.account.is_some()
        || !Path::new(&stdio.command).is_absolute()
        || stdio
            .cwd
            .as_ref()
            .is_some_and(|s| !Path::new(s).is_absolute())
        || !stdio.env.is_empty()
        || stdio.args.len() != 5
        || stdio.args[0] != "stdio"
        || !stdio.args.contains(&format!("--gh-host=https://{host}"))
        || !stdio.args.iter().any(|a| a == "--read-only")
        || !stdio.args.iter().any(|a| a == "--toolsets=")
    {
        return false;
    }
    let Some(tools) = stdio.args.iter().find_map(|s| s.strip_prefix("--tools=")) else {
        return false;
    };
    let names: Vec<_> = tools.split(',').map(str::to_owned).collect();
    let selected: BTreeSet<_> = names.iter().cloned().collect();
    if selected.len() != names.len()
        || selected.is_empty()
        || selected != access.allowed_operations
        || !operations.is_subset(&selected)
        || selected
            .iter()
            .any(|s| !KNOWN_OPERATIONS.contains(&s.as_str()))
        || config.tool_names.keys().any(|s| !selected.contains(s))
    {
        return false;
    }
    match &config.connector {
        Some(binding) => {
            // Do not infer read-only or destination declarations from names.
            if !stdio.secret_env.is_empty()
                || binding.account.is_empty()
                || binding.operations.keys().cloned().collect::<BTreeSet<_>>() != selected
                || binding
                    .operations
                    .values()
                    .any(|op| !op.read_only || op.destination_origin != format!("https://{host}"))
            {
                return false;
            }
        }
        None => {
            if stdio.secret_env
                != ["GITHUB_PERSONAL_ACCESS_TOKEN".to_owned()]
                    .into_iter()
                    .collect()
            {
                return false;
            }
        }
    }
    true
}

impl CollectionProgram for BoundGheProgram {
    fn accepts_target(&self, target: &CollectionTarget<'_>) -> bool {
        self.binding.matches(target)
    }
    fn start(&self, target: &CollectionTarget<'_>) -> Box<dyn CollectionCursor> {
        let state = if self.binding.matches(target) {
            self.spec
                .build(&self.repository, self.limits)
                .map(State::Fresh)
                .unwrap_or(State::Ended)
        } else {
            State::Ended
        };
        let final_read = match &state {
            State::Fresh(engine)
                if matches!(
                    &self.spec,
                    ProgramSpec::PullRequestChanges { .. }
                        | ProgramSpec::WorkflowRunJobs { .. }
                        | ProgramSpec::IssueComments { .. }
                ) =>
            {
                engine.next().cloned().map(|call| FinalRead {
                    call,
                    observation: None,
                })
            }
            _ => None,
        };
        Box::new(Cursor {
            binding: self.binding.clone(),
            history: self.history.clone(),
            limits: self.limits,
            state,
            context: None,
            observations: Vec::new(),
            bytes: 0,
            final_read,
        })
    }
}
enum State {
    Fresh(Engine),
    Waiting(Engine),
    Ended,
}
struct Cursor {
    binding: Arc<Binding>,
    history: Arc<HistoricalStore>,
    limits: ProgramLimits,
    state: State,
    context: Option<Value>,
    observations: Vec<HistoricalObservation>,
    bytes: usize,
    final_read: Option<FinalRead>,
}
// Only an actual committed and joined first read can provide this handle.
// It stays in the original cursor, never in the historical data or wire format.
struct FinalRead {
    call: ReadCall,
    observation: Option<ReadObservation>,
}
impl FinalRead {
    fn matches(&self, call: &ReadCall) -> bool {
        self.call.operation() == call.operation() && self.call.arguments() == call.arguments()
    }
}
fn next_call(engine: &Engine, read_only: bool) -> Option<CollectionCall> {
    let call = engine.next()?;
    let constructor = if read_only {
        CollectionCall::read_only
    } else {
        CollectionCall::new
    };
    constructor(call.operation().into(), call.arguments().clone()).ok()
}
impl CollectionCursor for Cursor {
    fn begin(&mut self) -> CollectionStep {
        let State::Fresh(engine) = std::mem::replace(&mut self.state, State::Ended) else {
            return CollectionStep::Refuse;
        };
        let Some(call) = next_call(&engine, self.final_read.is_some()) else {
            return CollectionStep::Refuse;
        };
        self.state = State::Waiting(engine);
        CollectionStep::Call(call)
    }
    fn observe(&mut self, result: &CommittedCollectionResult<'_>) -> CollectionStep {
        // Every early return is terminal for this cursor, including decode or
        // storage refusal. Never leave the pending call available for a retry.
        let State::Waiting(mut engine) = std::mem::replace(&mut self.state, State::Ended) else {
            return CollectionStep::Refuse;
        };
        if !self.binding.matches(result.target()) {
            return CollectionStep::Refuse;
        }
        let Some(call) = engine.next().cloned() else {
            return CollectionStep::Refuse;
        };
        if call.operation() != result.operation()
            || call.arguments() != result.arguments()
            || !self.binding.operations.contains(call.operation())
            || !result.result().settlement_failures.is_empty()
            || self.observations.len() >= usize::from(self.limits.calls)
        {
            return CollectionStep::Refuse;
        }
        let blocks = result.result().content.iter().map(|b| match b {
            ContentBlock::Text { text } => ProjectedBlock::Text(text),
            ContentBlock::Structured { data } => ProjectedBlock::Structured(data.get()),
            _ => ProjectedBlock::Unsupported,
        });
        let Ok(payload) = projected_github_payload(&call, result.result().is_error, blocks) else {
            return CollectionStep::Refuse;
        };
        let native = result.native_observation();
        // Correlation copied only from the actual native object, never from a
        // response body or restored JSON. This is an additional equality check.
        let context = json!({"application":native.application_id,"instance":native.instance_id,
            "login_scope":native.login_scope_id,"viewer":native.viewer_id,"origin":native.origin,
            "realm":native.realm,"session":native.session_id,"job":native.job_id,"server":native.server,"agent":native.agent});
        if self.context.as_ref().is_some_and(|old| old != &context) {
            return CollectionStep::Refuse;
        }
        let Ok(audit) = serde_json::to_value(native) else {
            return CollectionStep::Refuse;
        };
        let added = payload
            .bytes()
            .len()
            .checked_add(payload.structured_companion.map_or(0, str::len))
            .and_then(|n| n.checked_add(result.operation().len()))
            .and_then(|n| n.checked_add(result.arguments().to_string().len()))
            .and_then(|n| n.checked_add(audit.to_string().len()));
        let Some(bytes) = added.and_then(|n| self.bytes.checked_add(n)) else {
            return CollectionStep::Refuse;
        };
        if bytes > self.limits.result_bytes {
            return CollectionStep::Refuse;
        }
        let Ok(data) = engine.observe(&call, payload.bytes()) else {
            return CollectionStep::Refuse;
        };
        if self.observations.is_empty()
            && let Some(final_read) = &mut self.final_read
        {
            if !final_read.matches(&call) {
                return CollectionStep::Refuse;
            }
            let Some(observation) = result.read_observation() else {
                return CollectionStep::Refuse;
            };
            final_read.observation = Some(observation);
        }
        // The decoder borrows UTF-8 native strings, so this conversion cannot
        // create a replacement character or silently change result bytes.
        let Ok(text) = std::str::from_utf8(payload.bytes()) else {
            return CollectionStep::Refuse;
        };
        self.observations.push(HistoricalObservation {
            operation: result.operation().into(),
            arguments: result.arguments().clone(),
            native: audit,
            payload: text.into(),
            structured_companion: payload.structured_companion.map(str::to_owned),
        });
        self.bytes = bytes;
        self.context = Some(context.clone());
        if let Some(collection) = data {
            let history = ParsedHistory {
                source_id: self.binding.source_id.clone(),
                requirement_id: self.binding.requirement_id.clone(),
                resource: self.binding.resource.clone(),
                context,
                collection,
                observations: std::mem::take(&mut self.observations),
                bytes,
            };
            return if self.history.retain(history).is_ok() {
                CollectionStep::Finish(ResourceAssessment::Verified)
            } else {
                CollectionStep::Refuse
            };
        }
        let Some(pending) = engine.next() else {
            return CollectionStep::Refuse;
        };
        let step = if let Some(final_read) = &mut self.final_read
            && final_read.matches(pending)
        {
            // Take the original handle exactly once. No new call, altered
            // arguments or replacement requirement can bypass a missing handle.
            let Some(observation) = final_read.observation.take() else {
                return CollectionStep::Refuse;
            };
            CollectionStep::ReobserveRead(observation)
        } else {
            let Some(next) = next_call(&engine, false) else {
                return CollectionStep::Refuse;
            };
            CollectionStep::Call(next)
        };
        self.state = State::Waiting(engine);
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::mcp_config::{McpAccountDeclaration, McpServerAccess, McpStdioConfig};
    use std::collections::{BTreeMap, HashMap};

    fn config() -> McpServerConfig {
        McpServerConfig {
            name: "selected-ghe".into(),
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                command: "/installed/github-mcp-server".into(),
                args: vec![
                    "stdio".into(),
                    "--gh-host=https://ghe.example.test".into(),
                    "--read-only".into(),
                    "--toolsets=".into(),
                    "--tools=pull_request_read".into(),
                ],
                env: HashMap::new(),
                secret_env: ["GITHUB_PERSONAL_ACCESS_TOKEN".into()]
                    .into_iter()
                    .collect(),
                cwd: None,
            }),
            connect_timeout_secs: Some(10),
            tool_names: BTreeMap::new(),
            access: Some(McpServerAccess {
                allowed_operations: ["pull_request_read".into()].into_iter().collect(),
            }),
            connector: None,
            account: None,
        }
    }
    fn repo() -> GheRepository {
        GheRepository {
            host: "ghe.example.test".into(),
            owner: "team".into(),
            repository: "project".into(),
        }
    }
    fn limits() -> ProgramLimits {
        ProgramLimits {
            calls: 8,
            result_bytes: 1048576,
        }
    }
    fn bind(config: McpServerConfig) -> Result<BoundGheProgram, AdapterError> {
        BoundGheProgram::new(
            "original-source".into(),
            "original-requirement".into(),
            config,
            repo(),
            ProgramSpec::PullRequestStatuses {
                number: 17,
                head: None,
            },
            limits(),
            Arc::new(HistoricalStore::default()),
        )
    }
    fn stdio(config: &mut McpServerConfig) -> &mut McpStdioConfig {
        match &mut config.transport {
            McpTransportConfig::Stdio(s) => s,
            _ => panic!("fixture"),
        }
    }

    #[test]
    fn target_selection_retains_the_original_ids_and_derived_exact_resource() {
        let p = bind(config()).unwrap();
        assert_eq!(p.source_id(), "original-source");
        assert_eq!(p.binding.requirement_id, "original-requirement");
        assert_eq!(p.binding.resource["selector"]["number"], 17);
        assert_eq!(p.binding.resource["host"], "ghe.example.test");
        assert_eq!(p.binding.installed, config());
        assert!(Arc::new(p).requirement().is_ok());
    }
    #[test]
    fn wrong_host_plain_environment_or_unbounded_toolsets_are_rejected() {
        let edits: [fn(&mut McpStdioConfig); 5] = [
            |s| s.args[1] = "--gh-host=https://foreign.example.test".into(),
            |s| {
                s.env
                    .insert("GITHUB_HOST".into(), "https://foreign.example.test".into());
            },
            |s| s.args[3] = "--toolsets=all".into(),
            |s| s.args.push("--insecure".into()),
            |s| s.command = "relative-binary".into(),
        ];
        for edit in edits {
            let mut c = config();
            edit(stdio(&mut c));
            assert!(matches!(bind(c), Err(AdapterError::InvalidSelection)));
        }
    }
    #[test]
    fn account_absence_is_not_changed_into_a_no_account_declaration() {
        let mut c = config();
        c.account = Some(McpAccountDeclaration::NoAccount);
        assert!(matches!(bind(c), Err(AdapterError::InvalidSelection)));
        let mut c = config();
        stdio(&mut c).secret_env.clear();
        assert!(matches!(bind(c), Err(AdapterError::InvalidSelection)));
    }
    #[test]
    fn launch_and_native_access_ceilings_must_agree_without_unknown_operations() {
        let mut c = config();
        c.access = None;
        assert!(matches!(bind(c), Err(AdapterError::InvalidSelection)));
        let mut c = config();
        stdio(&mut c).args[4] = "--tools=pull_request_read,pull_request_read".into();
        assert!(matches!(bind(c), Err(AdapterError::InvalidSelection)));
        let mut c = config();
        stdio(&mut c).args[4] = "--tools=pull_request_read,issue_write".into();
        c.access
            .as_mut()
            .unwrap()
            .allowed_operations
            .insert("issue_write".into());
        assert!(matches!(bind(c), Err(AdapterError::InvalidSelection)));
        let mut c = config();
        c.tool_names
            .insert("foreign_operation".into(), "alias".into());
        assert!(matches!(bind(c), Err(AdapterError::InvalidSelection)));
    }
    #[test]
    fn invalid_ids_selectors_and_limits_are_rejected_before_native_admission() {
        for (number, limits) in [
            (0, limits()),
            (
                17,
                ProgramLimits {
                    calls: 1,
                    result_bytes: 1048576,
                },
            ),
            (
                17,
                ProgramLimits {
                    calls: 8,
                    result_bytes: 0,
                },
            ),
        ] {
            assert!(
                BoundGheProgram::new(
                    "source".into(),
                    "required".into(),
                    config(),
                    repo(),
                    ProgramSpec::PullRequestStatuses { number, head: None },
                    limits,
                    Arc::new(HistoricalStore::default())
                )
                .is_err()
            );
        }
        assert!(
            BoundGheProgram::new(
                "".into(),
                "required".into(),
                config(),
                repo(),
                ProgramSpec::PullRequestStatuses {
                    number: 17,
                    head: None
                },
                limits(),
                Arc::new(HistoricalStore::default())
            )
            .is_err()
        );
    }
    #[test]
    fn catalogue_builds_fresh_programs_with_their_own_operation_ceilings() {
        use forge_ghe_collections::{DiscussionKind, IssueQuery, PullRequestQuery};
        let head = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned();
        let cases = vec![
            ProgramSpec::PullRequestStatuses {
                number: 17,
                head: Some(head.clone()),
            },
            ProgramSpec::PullRequestThreads { number: 17 },
            ProgramSpec::PullRequestChanges {
                number: 17,
                head: Some(head.clone()),
            },
            ProgramSpec::PullRequestDiscussion {
                number: 17,
                kind: DiscussionKind::Comments,
            },
            ProgramSpec::PullRequestDiscussion {
                number: 17,
                kind: DiscussionKind::Reviews,
            },
            ProgramSpec::History {
                head: Some(head.clone()),
                since: "2026-10-20T08:00:00Z".into(),
                until: "2026-10-27T08:00:00Z".into(),
            },
            ProgramSpec::History {
                head: None,
                since: "2026-10-20T08:00:00Z".into(),
                until: "2026-10-27T08:00:00Z".into(),
            },
            ProgramSpec::WorkflowRuns,
            ProgramSpec::WorkflowRunJobs {
                run_id: 17,
                attempt: 2,
                head,
            },
            ProgramSpec::Issues {
                query: IssueQuery::Open,
            },
            ProgramSpec::IssueComments { number: 17 },
            ProgramSpec::PullRequests {
                query: PullRequestQuery::All,
            },
        ];
        for spec in cases {
            let first = spec.build(&repo(), limits()).unwrap();
            let second = spec.build(&repo(), limits()).unwrap();
            let call = first.next().unwrap();
            assert!(spec.operations().iter().any(|op| op == call.operation()));
            assert_eq!(call.arguments(), second.next().unwrap().arguments());
            assert_eq!(spec.resource(&repo(), limits())["program"], spec.id());
        }
    }
    #[test]
    fn repeated_begin_cannot_reissue_the_original_call() {
        let p = bind(config()).unwrap();
        let mut c = Cursor {
            state: State::Fresh(p.spec.build(&p.repository, p.limits).unwrap()),
            binding: p.binding,
            history: p.history,
            limits: p.limits,
            context: None,
            observations: Vec::new(),
            bytes: 0,
            final_read: None,
        };
        assert!(matches!(c.begin(), CollectionStep::Call(_)));
        assert!(matches!(c.begin(), CollectionStep::Refuse));
        assert!(matches!(c.begin(), CollectionStep::Refuse));
    }
}
