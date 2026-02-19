use crate::error::{MobError, MobResult};
use crate::model::{
    CollectionPolicy, DispatchMode, MobId, MobSpec, MobSpecBody, MobSpecDocument, MobSpecRecord,
    MobSpecRevision, SpecUpdateMode, StepId,
};
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ApplyContext {
    pub context_root: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
    pub now: DateTime<Utc>,
}

impl Default for ApplyContext {
    fn default() -> Self {
        Self {
            context_root: None,
            base_dir: None,
            now: Utc::now(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ApplySpecRequest {
    pub spec_toml: String,
    pub requested_mob_id: Option<String>,
    pub expected_revision: Option<MobSpecRevision>,
    pub update_mode: SpecUpdateMode,
    pub context: ApplyContext,
}

pub struct SpecValidator;

impl SpecValidator {
    pub fn new() -> Self {
        Self
    }

    pub async fn validate_toml(&self, request: &ApplySpecRequest) -> MobResult<Vec<MobSpecRecord>> {
        let raw_doc: toml::Value = toml::from_str(&request.spec_toml)
            .map_err(|err| MobError::SpecParse(err.to_string()))?;
        reject_namespace_overrides(&raw_doc)?;
        let mut doc: MobSpecDocument = raw_doc
            .try_into()
            .map_err(|err| MobError::SpecParse(err.to_string()))?;

        if doc.mob.specs.is_empty() {
            return Err(MobError::validation("mob.specs", "must not be empty"));
        }

        let mut validated = Vec::new();
        for (mob_id, mut body) in std::mem::take(&mut doc.mob.specs) {
            if let Some(requested) = &request.requested_mob_id
                && requested != &mob_id
            {
                continue;
            }

            self.validate_spec_body(&mob_id, &mut body, &request.context)
                .await?;
            let spec = MobSpec {
                revision: body.revision.unwrap_or(1),
                roles: body.roles,
                topology: body.topology,
                flows: body.flows,
                prompts: body.prompts,
                schemas: body.schemas,
                tool_bundles: body.tool_bundles,
                resolvers: body.resolvers,
                supervisor: body.supervisor,
                limits: body.limits,
                retention: body.retention,
                applied_at: request.context.now,
            };
            validated.push(MobSpecRecord {
                mob_id: MobId::from(mob_id),
                spec,
            });
        }

        if validated.is_empty() {
            return Err(MobError::validation(
                "requested_mob_id",
                "requested mob_id was not found in input",
            ));
        }

        Ok(validated)
    }

    async fn validate_spec_body(
        &self,
        mob_id: &str,
        body: &mut MobSpecBody,
        context: &ApplyContext,
    ) -> MobResult<()> {
        if body.roles.is_empty() {
            return Err(MobError::validation("roles", "must not be empty"));
        }
        if body.flows.is_empty() {
            return Err(MobError::validation("flows", "must not be empty"));
        }

        self.validate_roles(mob_id, body, context).await?;
        self.validate_topology_roles(body)?;
        self.validate_flows(body)?;

        Ok(())
    }

    async fn validate_roles(
        &self,
        _mob_id: &str,
        body: &mut MobSpecBody,
        context: &ApplyContext,
    ) -> MobResult<()> {
        let role_names: HashSet<String> = body.roles.keys().cloned().collect();

        for (role_name, role) in &mut body.roles {
            if role.prompt_ref.trim().is_empty() {
                return Err(MobError::validation(
                    &format!("roles.{role_name}.prompt_ref"),
                    "must not be empty",
                ));
            }

            role.prompt = resolve_prompt(&role.prompt_ref, &body.prompts, context).await?;

            if matches!(
                role.cardinality,
                crate::model::CardinalityKind::PerKey | crate::model::CardinalityKind::PerMeerkat
            ) && role.resolver.is_none()
            {
                return Err(MobError::validation(
                    &format!("roles.{role_name}.resolver"),
                    "is required for per_key/per_meerkat cardinality",
                ));
            }

            for bundle_id in &role.tool_bundles {
                if !body.tool_bundles.contains_key(bundle_id) {
                    return Err(MobError::validation(
                        &format!("roles.{role_name}.tool_bundles"),
                        &format!("references unknown bundle '{bundle_id}'"),
                    ));
                }
            }

            if let Some(resolver_id) = &role.resolver
                && !body.resolvers.contains_key(resolver_id)
            {
                return Err(MobError::validation(
                    &format!("roles.{role_name}.resolver"),
                    &format!("references unknown resolver '{resolver_id}'"),
                ));
            }
        }

        for (resolver_id, resolver) in &body.resolvers {
            if resolver.kind.trim().is_empty() {
                return Err(MobError::validation(
                    &format!("resolvers.{resolver_id}.kind"),
                    "must not be empty",
                ));
            }
        }

        for role in &role_names {
            if !body.roles.contains_key(role) {
                return Err(MobError::validation("roles", "unexpected role validation state"));
            }
        }

        Ok(())
    }

    fn validate_topology_roles(&self, body: &MobSpecBody) -> MobResult<()> {
        let role_names: HashSet<&str> = body.roles.keys().map(String::as_str).collect();
        let validate_role_refs =
            |prefix: &str, from_roles: &[String], to_roles: &[String]| -> MobResult<()> {
            for role in from_roles {
                if role != "*" && !role_names.contains(role.as_str()) {
                    return Err(MobError::validation(
                        &format!("{prefix}.from_roles"),
                        &format!("unknown role '{role}'"),
                    ));
                }
            }
            for role in to_roles {
                if role != "*" && !role_names.contains(role.as_str()) {
                    return Err(MobError::validation(
                        &format!("{prefix}.to_roles"),
                        &format!("unknown role '{role}'"),
                    ));
                }
            }
            Ok(())
        };

        for (index, rule) in body.topology.flow_dispatched.rules.iter().enumerate() {
            validate_role_refs(
                &format!("topology.flow_dispatched.rules.{index}"),
                &rule.from_roles,
                &rule.to_roles,
            )?;
        }

        for (index, rule) in body.topology.ad_hoc.rules.iter().enumerate() {
            validate_role_refs(
                &format!("topology.ad_hoc.rules.{index}"),
                &rule.from_roles,
                &rule.to_roles,
            )?;
        }

        Ok(())
    }

    fn validate_flows(&self, body: &MobSpecBody) -> MobResult<()> {
        for (flow_id, flow) in &body.flows {
            if flow.steps.is_empty() {
                return Err(MobError::validation(
                    &format!("flows.{flow_id}.steps"),
                    "must not be empty",
                ));
            }

            let mut step_ids: IndexMap<StepId, usize> = IndexMap::new();
            for (index, step) in flow.steps.iter().enumerate() {
                if step.step_id.as_ref().trim().is_empty() {
                    return Err(MobError::validation(
                        &format!("flows.{flow_id}.steps[{index}].step_id"),
                        "must not be empty",
                    ));
                }
                if step_ids.insert(step.step_id.clone(), index).is_some() {
                    return Err(MobError::validation(
                        &format!("flows.{flow_id}.steps[{index}].step_id"),
                        &format!("duplicate step_id '{}': must be unique", step.step_id),
                    ));
                }

                if step.targets.role.as_ref().trim().is_empty() {
                    return Err(MobError::validation(
                        &format!("flows.{flow_id}.steps[{index}].targets.role"),
                        "must not be empty",
                    ));
                }

                if !body.roles.contains_key(step.targets.role.as_ref()) {
                    return Err(MobError::validation(
                        &format!("flows.{flow_id}.steps[{index}].targets.role"),
                        &format!("unknown role '{}'", step.targets.role),
                    ));
                }

                if matches!(step.dispatch_mode, DispatchMode::FanIn) {
                    return Err(MobError::validation(
                        &format!("flows.{flow_id}.steps[{index}].dispatch_mode"),
                        "fan_in is reserved in v1 and must not be used",
                    ));
                }

                if let Some(schema_ref) = &step.expected_schema_ref
                    && !body.schemas.contains_key(schema_ref)
                {
                    return Err(MobError::validation(
                        &format!("flows.{flow_id}.steps[{index}].expected_schema_ref"),
                        &format!("unknown schema ref '{schema_ref}'"),
                    ));
                }

                if let CollectionPolicy::Quorum { n } = step.collection_policy
                    && n == 0
                {
                    return Err(MobError::validation(
                        &format!("flows.{flow_id}.steps[{index}].collection_policy"),
                        "quorum(n) requires n >= 1",
                    ));
                }
            }

            for step in &flow.steps {
                for dep in &step.depends_on {
                    if !step_ids.contains_key(dep) {
                        return Err(MobError::validation(
                            &format!("flows.{flow_id}.steps.{}.depends_on", step.step_id),
                            &format!("depends_on references unknown step '{dep}'"),
                        ));
                    }
                    if dep == &step.step_id {
                        return Err(MobError::validation(
                            &format!("flows.{flow_id}.steps.{}.depends_on", step.step_id),
                            "step cannot depend on itself",
                        ));
                    }
                }
            }

            validate_dag(
                flow_id,
                flow.steps
                    .iter()
                    .map(|step| (step.step_id.as_ref(), &step.depends_on)),
            )?;
            let depth = compute_dag_depth(&flow.steps);
            if depth > body.limits.max_flow_depth {
                return Err(MobError::validation(
                    &format!("flows.{flow_id}"),
                    &format!(
                        "flow depth {depth} exceeds limit {}",
                        body.limits.max_flow_depth
                    ),
                ));
            }
        }

        Ok(())
    }
}

impl Default for SpecValidator {
    fn default() -> Self {
        Self::new()
    }
}

fn reject_namespace_overrides(raw_doc: &toml::Value) -> MobResult<()> {
    let Some(specs) = raw_doc
        .get("mob")
        .and_then(|mob| mob.get("specs"))
        .and_then(toml::Value::as_table)
    else {
        return Ok(());
    };

    for (mob_id, spec_value) in specs {
        if spec_value
            .as_table()
            .and_then(|table| table.get("namespace"))
            .is_some()
        {
            return Err(MobError::validation(
                &format!("mob.specs.{mob_id}.namespace"),
                "namespace override is not allowed in v1; namespace is computed as {realm_id}/{mob_id}",
            ));
        }
    }

    Ok(())
}

fn compute_dag_depth(steps: &[crate::model::FlowStepSpec]) -> u32 {
    let mut depth: HashMap<String, u32> = HashMap::new();
    let mut changed = true;

    while changed {
        changed = false;
        for step in steps {
            let next_depth = if step.depends_on.is_empty() {
                1
            } else {
                let max_parent = step
                    .depends_on
                    .iter()
                    .map(|dep| depth.get(dep.as_ref()).copied().unwrap_or(1))
                    .max()
                    .unwrap_or(1);
                max_parent + 1
            };
            if depth.get(step.step_id.as_ref()).copied().unwrap_or(0) != next_depth {
                depth.insert(step.step_id.to_string(), next_depth);
                changed = true;
            }
        }
    }

    depth.values().copied().max().unwrap_or(0)
}

fn validate_dag<'a, I>(flow_id: &str, steps: I) -> MobResult<()>
where
    I: IntoIterator<Item = (&'a str, &'a Vec<StepId>)>,
{
    let mut indegree: HashMap<String, usize> = HashMap::new();
    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();

    for (id, deps) in steps {
        indegree.entry(id.to_string()).or_insert(0);
        for dep in deps {
            adjacency
                .entry(dep.to_string())
                .or_default()
                .push(id.to_string());
            *indegree.entry(id.to_string()).or_insert(0) += 1;
            indegree.entry(dep.to_string()).or_insert(0);
        }
    }

    let mut queue = VecDeque::new();
    for (node, degree) in &indegree {
        if *degree == 0 {
            queue.push_back(node.clone());
        }
    }

    let mut visited = 0usize;
    while let Some(node) = queue.pop_front() {
        visited += 1;
        if let Some(children) = adjacency.get(&node) {
            for child in children {
                if let Some(entry) = indegree.get_mut(child) {
                    *entry -= 1;
                    if *entry == 0 {
                        queue.push_back(child.clone());
                    }
                }
            }
        }
    }

    if visited != indegree.len() {
        return Err(MobError::validation(
            &format!("flows.{flow_id}.steps"),
            "cycle detected in depends_on graph",
        ));
    }

    Ok(())
}

async fn resolve_prompt(
    prompt_ref: &str,
    prompts: &BTreeMap<String, String>,
    context: &ApplyContext,
) -> MobResult<String> {
    if let Some(name) = prompt_ref.strip_prefix("config://prompts/") {
        let key = name.trim();
        if key.is_empty() {
            return Err(MobError::validation(
                "prompt_ref",
                "config://prompts/<name> requires a non-empty name",
            ));
        }
        let prompt = prompts.get(key).ok_or_else(|| {
            MobError::validation(
                "prompt_ref",
                &format!("prompt '{key}' not found in [prompts] map"),
            )
        })?;
        return Ok(prompt.clone());
    }

    if let Some(path) = prompt_ref.strip_prefix("file://") {
        let base = context
            .base_dir
            .as_ref()
            .or(context.context_root.as_ref())
            .ok_or_else(|| {
                MobError::validation(
                    "prompt_ref",
                    "file:// prompt refs require apply base_dir or context_root",
                )
            })?;
        let resolved = normalize_file_ref(path, base)?;
        let content = tokio::fs::read_to_string(&resolved).await.map_err(|err| {
            MobError::validation(
                "prompt_ref",
                &format!("failed to read prompt file '{}': {err}", resolved.display()),
            )
        })?;
        return Ok(content);
    }

    Err(MobError::validation(
        "prompt_ref",
        "unsupported scheme, expected config://prompts/<name> or file://<path>",
    ))
}

fn normalize_file_ref(path: &str, base: &Path) -> MobResult<PathBuf> {
    let raw = PathBuf::from(path);
    if raw.is_absolute() {
        return Ok(raw);
    }
    Ok(base.join(raw))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn apply_request(spec_toml: &str) -> ApplySpecRequest {
        ApplySpecRequest {
            spec_toml: spec_toml.to_string(),
            requested_mob_id: None,
            expected_revision: None,
            update_mode: SpecUpdateMode::DrainReplace,
            context: ApplyContext::default(),
        }
    }

    #[tokio::test]
    async fn rejects_fan_in_dispatch_mode() {
        let toml = r#"
[mob.specs.invoice.roles.coordinator]
prompt_ref = "config://prompts/coordinator"
cardinality = "singleton"

[mob.specs.invoice.topology.flow_dispatched]
mode = "advisory"

[mob.specs.invoice.prompts]
coordinator = "You coordinate"

[[mob.specs.invoice.flows.triage.steps]]
step_id = "s1"
dispatch_mode = "fan_in"
[mob.specs.invoice.flows.triage.steps.targets]
role = "coordinator"
"#;

        let validator = SpecValidator::new();
        let err = validator.validate_toml(&apply_request(toml)).await.unwrap_err();
        assert!(err.to_string().contains("fan_in is reserved"));
    }

    #[tokio::test]
    async fn rejects_namespace_override() {
        let toml = r#"
[mob.specs.invoice]
namespace = "manual-namespace"

[mob.specs.invoice.roles.coordinator]
prompt_ref = "config://prompts/coordinator"
cardinality = "singleton"

[mob.specs.invoice.topology.flow_dispatched]
mode = "advisory"

[mob.specs.invoice.prompts]
coordinator = "You coordinate"

[[mob.specs.invoice.flows.triage.steps]]
step_id = "s1"
dispatch_mode = "one_to_one"
[mob.specs.invoice.flows.triage.steps.targets]
role = "coordinator"
"#;

        let validator = SpecValidator::new();
        let err = validator.validate_toml(&apply_request(toml)).await.unwrap_err();
        assert!(err.to_string().contains("namespace override is not allowed"));
    }

    #[tokio::test]
    async fn resolves_config_prompts_into_role_prompt() {
        let toml = r#"
[mob.specs.invoice.roles.coordinator]
prompt_ref = "config://prompts/coordinator"
cardinality = "singleton"

[mob.specs.invoice.topology.flow_dispatched]
mode = "advisory"

[mob.specs.invoice.prompts]
coordinator = "You coordinate invoices"

[[mob.specs.invoice.flows.triage.steps]]
step_id = "s1"
dispatch_mode = "one_to_one"
[mob.specs.invoice.flows.triage.steps.targets]
role = "coordinator"
"#;

        let validator = SpecValidator::new();
        let specs = validator.validate_toml(&apply_request(toml)).await.unwrap();
        assert_eq!(specs.len(), 1);
        let role = specs[0].spec.roles.get("coordinator").unwrap();
        assert_eq!(role.prompt, "You coordinate invoices");
    }

    #[tokio::test]
    async fn rejects_missing_file_prompt_base() {
        let toml = r#"
[mob.specs.invoice.roles.coordinator]
prompt_ref = "file://prompts/coordinator.md"
cardinality = "singleton"

[mob.specs.invoice.topology.flow_dispatched]
mode = "advisory"

[[mob.specs.invoice.flows.triage.steps]]
step_id = "s1"
dispatch_mode = "one_to_one"
[mob.specs.invoice.flows.triage.steps.targets]
role = "coordinator"
"#;

        let validator = SpecValidator::new();
        let err = validator.validate_toml(&apply_request(toml)).await.unwrap_err();
        assert!(err.to_string().contains("file:// prompt refs require"));
    }

    #[tokio::test]
    async fn validates_dag_cycles() {
        let toml = r#"
[mob.specs.invoice.roles.coordinator]
prompt_ref = "config://prompts/coordinator"
cardinality = "singleton"

[mob.specs.invoice.topology.flow_dispatched]
mode = "advisory"

[mob.specs.invoice.prompts]
coordinator = "You coordinate invoices"

[[mob.specs.invoice.flows.triage.steps]]
step_id = "s1"
depends_on = ["s2"]
dispatch_mode = "one_to_one"
[mob.specs.invoice.flows.triage.steps.targets]
role = "coordinator"

[[mob.specs.invoice.flows.triage.steps]]
step_id = "s2"
depends_on = ["s1"]
dispatch_mode = "one_to_one"
[mob.specs.invoice.flows.triage.steps.targets]
role = "coordinator"
"#;

        let validator = SpecValidator::new();
        let err = validator.validate_toml(&apply_request(toml)).await.unwrap_err();
        assert!(err.to_string().contains("cycle detected"));
    }

    #[tokio::test]
    async fn rejects_quorum_zero() {
        let toml = r#"
[mob.specs.invoice.roles.coordinator]
prompt_ref = "config://prompts/coordinator"
cardinality = "singleton"

[mob.specs.invoice.topology.flow_dispatched]
mode = "advisory"

[mob.specs.invoice.prompts]
coordinator = "You coordinate invoices"

[[mob.specs.invoice.flows.triage.steps]]
step_id = "s1"
dispatch_mode = "one_to_one"
collection_policy = { kind = "quorum", n = 0 }
[mob.specs.invoice.flows.triage.steps.targets]
role = "coordinator"
"#;

        let validator = SpecValidator::new();
        let err = validator.validate_toml(&apply_request(toml)).await.unwrap_err();
        assert!(err.to_string().contains("quorum(n) requires n >= 1"));
    }

    #[tokio::test]
    async fn rejects_unknown_schema_ref() {
        let toml = r#"
[mob.specs.invoice.roles.coordinator]
prompt_ref = "config://prompts/coordinator"
cardinality = "singleton"

[mob.specs.invoice.topology.flow_dispatched]
mode = "advisory"

[mob.specs.invoice.prompts]
coordinator = "You coordinate invoices"

[[mob.specs.invoice.flows.triage.steps]]
step_id = "s1"
expected_schema_ref = "missing"
dispatch_mode = "one_to_one"
[mob.specs.invoice.flows.triage.steps.targets]
role = "coordinator"
"#;

        let validator = SpecValidator::new();
        let err = validator.validate_toml(&apply_request(toml)).await.unwrap_err();
        assert!(err.to_string().contains("unknown schema ref"));
    }

    #[tokio::test]
    async fn rejects_dynamic_cardinality_without_resolver() {
        let toml = r#"
[mob.specs.invoice.roles.coordinator]
prompt_ref = "config://prompts/coordinator"
cardinality = "per_key"

[mob.specs.invoice.topology.flow_dispatched]
mode = "advisory"

[mob.specs.invoice.prompts]
coordinator = "You coordinate invoices"

[[mob.specs.invoice.flows.triage.steps]]
step_id = "s1"
dispatch_mode = "one_to_one"
[mob.specs.invoice.flows.triage.steps.targets]
role = "coordinator"
"#;

        let validator = SpecValidator::new();
        let err = validator.validate_toml(&apply_request(toml)).await.unwrap_err();
        assert!(err.to_string().contains("resolver"));
    }

    #[tokio::test]
    async fn accepts_valid_parallel_dag_spec() {
        let toml = r#"
[mob.specs.invoice.roles.coordinator]
prompt_ref = "config://prompts/coordinator"
cardinality = "singleton"

[mob.specs.invoice.roles.reviewer]
prompt_ref = "config://prompts/reviewer"
cardinality = "singleton"

[mob.specs.invoice.topology.flow_dispatched]
mode = "advisory"

[mob.specs.invoice.prompts]
coordinator = "You coordinate invoices"
reviewer = "You review invoices"

[[mob.specs.invoice.flows.triage.steps]]
step_id = "dispatch"
dispatch_mode = "fan_out"
[mob.specs.invoice.flows.triage.steps.targets]
role = "reviewer"

[[mob.specs.invoice.flows.triage.steps]]
step_id = "synthesize"
depends_on = ["dispatch"]
dispatch_mode = "one_to_one"
[mob.specs.invoice.flows.triage.steps.targets]
role = "coordinator"
"#;

        let validator = SpecValidator::new();
        let specs = validator.validate_toml(&apply_request(toml)).await.unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].mob_id, "invoice".into());
    }
}
