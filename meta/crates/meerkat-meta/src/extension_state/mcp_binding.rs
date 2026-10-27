//! Host-owned document MCP composition. Only the public descriptor is durable.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use meerkat_core::mcp_config::McpTransportConfig;
use meerkat_core::types::ToolCallView;
use meerkat_core::{AgentToolDispatcher, McpServerConfig, ToolDispatchContext, ToolMutationClass};
use meerkat_mcp::{
    McpCallContext, McpCallContextError, McpCallContextProvider, McpCallTarget, McpResourceTarget,
};

use super::{ToolBundleFactory, ToolBundleRequirements};
pub use crate::document_mcp_marker::DOCUMENT_BINDING_ENV;
use crate::document_mcp_marker::is_marked as is_document_descriptor;
use crate::storage_predecessor::PREDECESSOR_DOCUMENT_BINDING_ENV;

pub use meerkat_mob_mcp::ChildToolBundleAvailability as ChildMcpAvailability;

/// An opt-in, process-local document service registration.
///
/// The public stdio descriptor contains the fixed executable, arguments and
/// canonical tool names. Its environment contains the nonsecret binding marker
/// and empty values declaring which launch fields the host may fill.
/// The runtime descriptor has the same executable, arguments and names, but
/// supplies the host-owned endpoint and credentials in its environment.
/// Neither the runtime descriptor nor the provider is serialized by Meta.
/// The host attests that public command and arguments contain no credentials.
pub struct HostMcpDocumentBinding {
    public_config: McpServerConfig,
    runtime_config: McpServerConfig,
    pub(crate) requirements: ToolBundleRequirements,
    pub(crate) factory: Arc<dyn ToolBundleFactory>,
    provider: Arc<dyn McpCallContextProvider>,
    pub(crate) child_availability: ChildMcpAvailability,
    validated_dispatcher: OnceLock<Arc<dyn AgentToolDispatcher>>,
}

impl HostMcpDocumentBinding {
    pub fn new(
        mut public_config: McpServerConfig,
        runtime_config: McpServerConfig,
        requirements: ToolBundleRequirements,
        factory: Arc<dyn ToolBundleFactory>,
        provider: Arc<dyn McpCallContextProvider>,
        child_availability: ChildMcpAvailability,
    ) -> Result<Self, String> {
        if requirements.namespace.is_empty()
            || requirements.read_tool.is_empty()
            || requirements.edit_tool.is_empty()
            || requirements.read_tool == requirements.edit_tool
        {
            return Err(
                "document MCP binding requires a namespace and distinct read/edit tools".into(),
            );
        }
        if !public_config.tool_names.is_empty() || !runtime_config.tool_names.is_empty() {
            return Err(
                "document MCP bindings require canonical raw tool names; aliases are unsupported"
                    .into(),
            );
        }
        let McpTransportConfig::Stdio(public_stdio) = &mut public_config.transport else {
            return Err("document MCP bindings currently require a stdio public descriptor".into());
        };
        if public_stdio.env.iter().any(|(key, value)| {
            if key == DOCUMENT_BINDING_ENV {
                value != &requirements.namespace
            } else {
                !value.is_empty()
            }
        }) {
            return Err("document MCP public environment must declare empty runtime slots".into());
        }
        public_stdio
            .env
            .insert(DOCUMENT_BINDING_ENV.into(), requirements.namespace.clone());
        let mut public_shape = public_config.clone();
        let mut runtime_shape = runtime_config.clone();
        let McpTransportConfig::Stdio(runtime_stdio) = &mut runtime_shape.transport else {
            return Err("document MCP runtime must use the public stdio transport".into());
        };
        if runtime_stdio.env.contains_key(DOCUMENT_BINDING_ENV)
            || runtime_stdio
                .env
                .contains_key(PREDECESSOR_DOCUMENT_BINDING_ENV)
        {
            return Err(
                "document MCP runtime environment must not contain the public marker".into(),
            );
        }
        for value in runtime_stdio.env.values_mut() {
            value.clear();
        }
        if let McpTransportConfig::Stdio(public_stdio) = &mut public_shape.transport {
            public_stdio.env.remove(DOCUMENT_BINDING_ENV);
        }
        if public_shape != runtime_shape {
            return Err(
                "document MCP runtime may resolve only host-owned launch environment".into(),
            );
        }
        Ok(Self {
            public_config,
            runtime_config,
            requirements,
            factory,
            provider,
            child_availability,
            validated_dispatcher: OnceLock::new(),
        })
    }

    /// Copy this descriptor into an authored profile. It is safe to persist.
    pub fn public_config(&self) -> &McpServerConfig {
        &self.public_config
    }

    /// Host-only exact destination selected by the context provider.
    pub fn runtime_config(&self) -> &McpServerConfig {
        &self.runtime_config
    }

    pub(crate) fn validate_dispatcher(
        &self,
        dispatcher: Arc<dyn AgentToolDispatcher>,
    ) -> Result<(), String> {
        validate_document_dispatcher(dispatcher.as_ref(), &self.requirements)?;
        self.validated_dispatcher
            .set(dispatcher)
            .map_err(|_| "document MCP binding has already been activated".into())
    }

    fn mutation_class(&self, config: &McpServerConfig, operation: &str) -> ToolMutationClass {
        if self.validated_dispatcher.get().is_none() || config != &self.runtime_config {
            return ToolMutationClass::Unknown;
        }
        if operation == self.requirements.read_tool {
            ToolMutationClass::ReadOnly
        } else if operation == self.requirements.edit_tool {
            ToolMutationClass::Mutating
        } else {
            ToolMutationClass::Unknown
        }
    }
}

pub(crate) fn validate_document_dispatcher(
    dispatcher: &dyn AgentToolDispatcher,
    requirements: &ToolBundleRequirements,
) -> Result<(), String> {
    let tools = dispatcher.tools();
    if !tools.iter().any(|tool| tool.name == requirements.read_tool)
        || !tools.iter().any(|tool| tool.name == requirements.edit_tool)
        || dispatcher.tool_mutation_class(&requirements.read_tool) != ToolMutationClass::ReadOnly
        || dispatcher.tool_mutation_class(&requirements.edit_tool) != ToolMutationClass::Mutating
    {
        return Err("document bundle must declare its read and edit tools accurately".into());
    }
    Ok(())
}

pub(crate) fn resolve_document_mcp_configs(
    configs: &mut [McpServerConfig],
    bindings: &[Arc<HostMcpDocumentBinding>],
) -> Result<(), String> {
    let mut names = BTreeSet::new();
    for persisted in configs.iter() {
        let config = with_current_marker(persisted);
        let selected = bindings
            .iter()
            .find(|binding| binding.public_config.name == config.name);
        if (selected.is_some() || is_document_descriptor(&config)) && !names.insert(&persisted.name)
        {
            return Err("duplicate MCP server name in member profile".into());
        }
        match selected {
            Some(binding) if binding.public_config == *config => {}
            Some(_) => {
                return Err(
                    "document MCP public descriptor does not match host registration".into(),
                );
            }
            None if is_document_descriptor(&config) => {
                return Err("selected document MCP host binding is unavailable".into());
            }
            None => {}
        }
    }
    // Validate the whole selection before installing any runtime secret.
    for config in configs {
        let resolved = bindings
            .iter()
            .find(|binding| binding.public_config == *with_current_marker(config))
            .map(|binding| binding.runtime_config.clone());
        if let Some(runtime) = resolved {
            *config = runtime;
        }
    }
    Ok(())
}

/// `config` with a predecessor-spelled marker under its current name, so a
/// descriptor a profile persisted before 0.9 matches the binding it was
/// copied from. Every other field is kept. A descriptor that carries both
/// spellings is left as it is: it matches no binding and refuses.
fn with_current_marker(config: &McpServerConfig) -> Cow<'_, McpServerConfig> {
    let McpTransportConfig::Stdio(stdio) = &config.transport else {
        return Cow::Borrowed(config);
    };
    if stdio.env.contains_key(DOCUMENT_BINDING_ENV) {
        return Cow::Borrowed(config);
    }
    let Some(namespace) = stdio.env.get(PREDECESSOR_DOCUMENT_BINDING_ENV) else {
        return Cow::Borrowed(config);
    };
    let mut current = config.clone();
    if let McpTransportConfig::Stdio(stdio) = &mut current.transport {
        stdio.env.remove(PREDECESSOR_DOCUMENT_BINDING_ENV);
        stdio
            .env
            .insert(DOCUMENT_BINDING_ENV.to_string(), namespace.clone());
    }
    Cow::Owned(current)
}

pub(crate) fn pre_build_hook(
    user_hook: Option<crate::mob_handle_runtime::PreBuildHook>,
    bindings: Vec<Arc<HostMcpDocumentBinding>>,
) -> crate::mob_handle_runtime::PreBuildHook {
    Arc::new(move |request| {
        let user_hook = user_hook.clone();
        let bindings = bindings.clone();
        Box::pin(async move {
            if let Some(hook) = user_hook {
                hook(request).await?;
            }
            if let Some(build) = request.build.as_mut() {
                resolve_document_mcp_configs(&mut build.mcp_servers, &bindings).map_err(
                    |detail| {
                        meerkat_core::service::SessionError::Agent(
                            meerkat_core::error::AgentError::InternalError(detail),
                        )
                    },
                )?;
            }
            Ok(())
        })
    })
}

pub(crate) struct DocumentMcpContextProvider {
    pub(crate) bindings: Vec<Arc<HostMcpDocumentBinding>>,
    pub(crate) fallback: Option<Arc<dyn McpCallContextProvider>>,
}

impl DocumentMcpContextProvider {
    fn is_document_resource_destination(&self, config: &McpServerConfig) -> bool {
        self.bindings
            .iter()
            .any(|binding| binding.runtime_config.name == config.name)
            || is_document_descriptor(config)
    }
}

#[async_trait]
impl McpCallContextProvider for DocumentMcpContextProvider {
    async fn prepare_resource(
        &self,
        target: McpResourceTarget<'_>,
        context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        // Document bindings support the validated tool pair, not resource reads.
        // Route by the selected server, never by a URI or a tool's credentials.
        if self.is_document_resource_destination(target.config) {
            return Err(McpCallContextError::Unavailable);
        }
        match &self.fallback {
            Some(provider) => provider.prepare_resource(target, context).await,
            None => Ok(None),
        }
    }

    fn resource_authorization_target(
        &self,
        target: McpResourceTarget<'_>,
    ) -> Option<meerkat_core::authorization::OwnerQualifiedTarget> {
        if self.is_document_resource_destination(target.config) {
            return None;
        }
        self.fallback
            .as_ref()
            .and_then(|provider| provider.resource_authorization_target(target))
    }

    async fn prepare(
        &self,
        target: McpCallTarget<'_>,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<Option<McpCallContext>, McpCallContextError> {
        if let Some(binding) = self
            .bindings
            .iter()
            .find(|binding| binding.runtime_config.name == target.config.name)
        {
            if binding.mutation_class(target.config, target.raw_operation)
                == ToolMutationClass::Unknown
            {
                return Err(McpCallContextError::Denied);
            }
            return require_selected_context(
                binding.provider.prepare(target, call, context).await?,
            );
        }
        if is_document_descriptor(target.config) {
            return Err(McpCallContextError::Denied);
        }
        match &self.fallback {
            Some(provider) => provider.prepare(target, call, context).await,
            None => Ok(None),
        }
    }

    fn tool_mutation_class(
        &self,
        config: &McpServerConfig,
        raw_operation: &str,
    ) -> ToolMutationClass {
        if let Some(binding) = self
            .bindings
            .iter()
            .find(|binding| binding.runtime_config.name == config.name)
        {
            return binding.mutation_class(config, raw_operation);
        }
        if is_document_descriptor(config) {
            return ToolMutationClass::Unknown;
        }
        self.fallback
            .as_ref()
            .map_or(ToolMutationClass::Unknown, |provider| {
                provider.tool_mutation_class(config, raw_operation)
            })
    }
}

fn require_selected_context(
    context: Option<McpCallContext>,
) -> Result<Option<McpCallContext>, McpCallContextError> {
    context.map(Some).ok_or(McpCallContextError::Unavailable)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::types::ToolDef;
    use meerkat_core::{ToolDispatchOutcome, ToolError};
    use std::collections::HashMap;

    struct Probe;
    #[async_trait]
    impl AgentToolDispatcher for Probe {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            ["documents_read", "documents_apply"]
                .into_iter()
                .map(|name| {
                    Arc::new(ToolDef::new(
                        name,
                        "Document fixture",
                        serde_json::json!({"type":"object"}),
                    ))
                })
                .collect::<Vec<_>>()
                .into()
        }
        fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
            match name {
                "documents_read" => ToolMutationClass::ReadOnly,
                "documents_apply" => ToolMutationClass::Mutating,
                _ => ToolMutationClass::Unknown,
            }
        }
        async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            panic!("registration must not execute a document operation")
        }
    }

    struct Provider;
    #[async_trait]
    impl McpCallContextProvider for Provider {
        async fn prepare(
            &self,
            _: McpCallTarget<'_>,
            _: ToolCallView<'_>,
            _: &ToolDispatchContext,
        ) -> Result<Option<McpCallContext>, McpCallContextError> {
            panic!("classification must not prepare a call")
        }
        fn tool_mutation_class(&self, _: &McpServerConfig, _: &str) -> ToolMutationClass {
            // The registration's validated dispatcher is the semantic owner.
            // A provider cannot declare an apply operation read-only.
            ToolMutationClass::ReadOnly
        }
    }

    fn configs(credential: &str) -> (McpServerConfig, McpServerConfig) {
        let public = McpServerConfig::stdio(
            "documents",
            "documents-fixture",
            vec!["mcp".into()],
            HashMap::from([
                ("DOCUMENT_ENDPOINT".into(), String::new()),
                ("DOCUMENT_TOKEN".into(), String::new()),
            ]),
        );
        let mut runtime = public.clone();
        let McpTransportConfig::Stdio(stdio) = &mut runtime.transport else {
            unreachable!()
        };
        stdio.env = HashMap::from([
            (
                "DOCUMENT_ENDPOINT".into(),
                "http://127.0.0.1:32123/mcp".into(),
            ),
            ("DOCUMENT_TOKEN".into(), credential.into()),
        ]);
        (public, runtime)
    }

    fn binding(
        public: McpServerConfig,
        runtime: McpServerConfig,
    ) -> Result<HostMcpDocumentBinding, String> {
        HostMcpDocumentBinding::new(
            public,
            runtime,
            ToolBundleRequirements::durable_documents(
                "documents-fixture",
                "documents_read",
                "documents_apply",
            ),
            Arc::new(|_: super::super::ToolBundleContext| {
                Ok(Arc::new(Probe) as Arc<dyn AgentToolDispatcher>)
            }),
            Arc::new(Provider),
            ChildMcpAvailability::ChildAvailable,
        )
    }

    #[test]
    fn only_declared_runtime_environment_is_resolved_and_public_profile_is_stable() {
        let (public, runtime) = configs("first-process-secret");
        let binding = Arc::new(binding(public, runtime.clone()).unwrap());
        let durable = binding.public_config().clone();
        let mut request = vec![durable.clone()];
        resolve_document_mcp_configs(&mut request, std::slice::from_ref(&binding)).unwrap();
        assert_eq!(request, vec![runtime]);
        let encoded = serde_json::to_string(&durable).unwrap();
        assert!(!encoded.contains("first-process-secret"));
        assert!(!encoded.contains("127.0.0.1"));
        assert!(resolve_document_mcp_configs(&mut [durable.clone()], &[]).is_err());

        let (public, mut runtime) = configs("second-process-secret");
        let McpTransportConfig::Stdio(stdio) = &mut runtime.transport else {
            unreachable!()
        };
        stdio.env.insert(
            "DOCUMENT_ENDPOINT".into(),
            "http://127.0.0.1:33210/mcp".into(),
        );
        let restored = Arc::new(binding_for_restore(public, runtime.clone()));
        assert_eq!(restored.public_config(), &durable);
        let mut request = vec![serde_json::from_str(&encoded).unwrap()];
        resolve_document_mcp_configs(&mut request, &[restored]).unwrap();
        assert_eq!(request, vec![runtime]);
        assert!(!encoded.contains("second-process-secret"));
    }

    fn binding_for_restore(
        public: McpServerConfig,
        runtime: McpServerConfig,
    ) -> HostMcpDocumentBinding {
        binding(public, runtime).unwrap()
    }

    /// A descriptor persisted before 0.9 carries the predecessor spelling of
    /// the public marker. It is the same host-attested descriptor: it
    /// resolves to its binding's runtime twin exactly as the current spelling
    /// does, and with no binding to resolve it, it refuses instead of
    /// launching an unbound service.
    #[test]
    fn a_predecessor_marked_descriptor_resolves_and_never_launches_unbound() {
        let (public, runtime) = configs("secret");
        let binding = Arc::new(binding(public, runtime.clone()).unwrap());
        let mut persisted = binding.public_config().clone();
        let McpTransportConfig::Stdio(stdio) = &mut persisted.transport else {
            unreachable!()
        };
        let namespace = stdio
            .env
            .remove(DOCUMENT_BINDING_ENV)
            .expect("the public descriptor carries the marker");
        stdio
            .env
            .insert("MOBKIT_MCP_DOCUMENT_BINDING".into(), namespace);

        let mut request = vec![persisted.clone()];
        resolve_document_mcp_configs(&mut request, std::slice::from_ref(&binding)).unwrap();
        assert_eq!(request, vec![runtime]);
        assert!(
            resolve_document_mcp_configs(&mut [persisted], &[]).is_err(),
            "an unresolved predecessor-marked descriptor refuses"
        );
    }

    #[test]
    fn registration_rejects_aliases_and_arbitrary_destination_rewrites() {
        let (public, runtime) = configs("secret");
        let mut alias = public.clone();
        alias
            .tool_names
            .insert("documents_apply".into(), "documents_read".into());
        assert!(binding(alias, runtime.clone()).is_err());
        for field in ["command", "args", "env", "timeout", "transport"] {
            let mut changed = runtime.clone();
            let McpTransportConfig::Stdio(stdio) = &mut changed.transport else {
                unreachable!()
            };
            match field {
                "command" => stdio.command = "another-binary".into(),
                "args" => stdio.args.push("another-mode".into()),
                "env" => {
                    stdio
                        .env
                        .insert("UNDECLARED_SECRET".into(), "secret".into());
                }
                "timeout" => changed.connect_timeout_secs = Some(999),
                "transport" => {
                    changed = McpServerConfig::streamable_http(
                        "documents",
                        "http://127.0.0.1/mcp",
                        HashMap::new(),
                    );
                }
                _ => unreachable!(),
            }
            assert!(binding(public.clone(), changed).is_err(), "{field}");
        }
    }

    #[test]
    fn mismatched_public_descriptor_never_receives_any_runtime_secret() {
        let (public, runtime) = configs("secret");
        let binding = Arc::new(binding(public, runtime).unwrap());
        for field in ["command", "args", "env", "alias", "transport"] {
            let mut changed = binding.public_config().clone();
            let McpTransportConfig::Stdio(stdio) = &mut changed.transport else {
                unreachable!()
            };
            match field {
                "command" => stdio.command = "another-binary".into(),
                "args" => stdio.args.push("another-mode".into()),
                "env" => {
                    stdio.env.insert("DOCUMENT_TOKEN".into(), "forged".into());
                }
                "alias" => {
                    changed
                        .tool_names
                        .insert("documents_read".into(), "different_read".into());
                }
                "transport" => {
                    changed = McpServerConfig::streamable_http(
                        "documents",
                        "http://127.0.0.1/mcp",
                        HashMap::new(),
                    );
                }
                _ => unreachable!(),
            }
            let mut request = vec![changed];
            let before = request.clone();
            assert!(
                resolve_document_mcp_configs(&mut request, std::slice::from_ref(&binding)).is_err(),
                "{field}"
            );
            assert_eq!(request, before, "no partial resolution on {field}");
        }
        let mut duplicate = vec![binding.public_config().clone(); 2];
        let before = duplicate.clone();
        assert!(
            resolve_document_mcp_configs(&mut duplicate, std::slice::from_ref(&binding)).is_err()
        );
        assert_eq!(duplicate, before);
        let unrelated = McpServerConfig::stdio("unrelated", "ordinary-mcp", vec![], HashMap::new());
        let mut request = vec![unrelated.clone()];
        resolve_document_mcp_configs(&mut request, &[binding]).unwrap();
        assert_eq!(request, vec![unrelated]);
    }

    #[test]
    fn trusted_classes_come_only_from_the_validated_pair_and_exact_runtime() {
        let (public, runtime) = configs("secret");
        let binding = Arc::new(binding(public, runtime.clone()).unwrap());
        let provider = DocumentMcpContextProvider {
            bindings: vec![binding.clone()],
            fallback: None,
        };
        assert_eq!(
            provider.tool_mutation_class(&runtime, "documents_read"),
            ToolMutationClass::Unknown
        );
        binding.validate_dispatcher(Arc::new(Probe)).unwrap();
        assert_eq!(
            provider.tool_mutation_class(&runtime, "documents_read"),
            ToolMutationClass::ReadOnly
        );
        assert_eq!(
            provider.tool_mutation_class(&runtime, "documents_apply"),
            ToolMutationClass::Mutating
        );
        assert_eq!(
            provider.tool_mutation_class(binding.public_config(), "documents_read"),
            ToolMutationClass::Unknown
        );
        let (_, changed_runtime) = configs("replacement-secret");
        assert_eq!(
            provider.tool_mutation_class(&changed_runtime, "documents_read"),
            ToolMutationClass::Unknown
        );
        assert_eq!(
            provider.tool_mutation_class(&runtime, "another_operation"),
            ToolMutationClass::Unknown
        );
        let read_only = meerkat_core::ToolExecutionPolicy::resolve(
            meerkat_core::ops::ToolAccessPolicy::ReadOnly,
        )
        .unwrap();
        assert!(read_only.permits_call(
            "documents_read",
            provider.tool_mutation_class(&runtime, "documents_read")
        ));
        assert!(!read_only.permits_call(
            "documents_apply",
            provider.tool_mutation_class(&runtime, "documents_apply")
        ));
    }

    #[test]
    fn selected_provider_cannot_decline_context_and_unrelated_classes_keep_fallback() {
        assert!(matches!(
            require_selected_context(None),
            Err(McpCallContextError::Unavailable)
        ));
        assert!(
            require_selected_context(Some(McpCallContext::new(Default::default(), ())))
                .unwrap()
                .is_some()
        );
        let (public, runtime) = configs("secret");
        let binding = Arc::new(binding(public, runtime).unwrap());
        let provider = DocumentMcpContextProvider {
            bindings: vec![binding.clone()],
            fallback: Some(Arc::new(Provider)),
        };
        let ordinary =
            McpServerConfig::stdio("ordinary", "ordinary-fixture", vec![], HashMap::new());
        assert_eq!(
            provider.tool_mutation_class(&ordinary, "ordinary_read"),
            ToolMutationClass::ReadOnly
        );
        assert_eq!(
            provider.tool_mutation_class(binding.public_config(), "documents_read"),
            ToolMutationClass::Unknown
        );
    }

    // Obtain the opaque id from an actual native connection, not a fabricated
    // resource authority. This fixture only implements the MCP handshake.
    async fn resource_connection(name: &str) -> meerkat_mcp::McpConnection {
        let script = r#"import json, sys
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    result = {"protocolVersion": "2024-11-05", "capabilities": {},
              "serverInfo": {"name": "resource-routing-fixture", "version": "1"}}
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
"#;
        let mut config = McpServerConfig::stdio(
            name,
            "python3",
            vec!["-u".into(), "-c".into(), script.into()],
            HashMap::new(),
        );
        config.connect_timeout_secs = Some(5);
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            meerkat_mcp::McpConnection::connect(&config),
        )
        .await
        .expect("resource fixture handshake must finish")
        .expect("native resource fixture connects")
    }

    async fn close_resource_connection(connection: meerkat_mcp::McpConnection) {
        tokio::time::timeout(std::time::Duration::from_secs(5), connection.close())
            .await
            .expect("resource fixture close must finish")
            .expect("native resource fixture closes");
    }

    struct ResourceFallback {
        config: McpServerConfig,
        connection_id: meerkat_mcp::McpConnectionId,
        context_address: usize,
        preparations: std::sync::atomic::AtomicUsize,
        authorizations: std::sync::atomic::AtomicUsize,
    }

    impl ResourceFallback {
        fn verify_destination(&self, target: &McpResourceTarget<'_>) {
            assert_eq!(target.config, &self.config);
            assert_eq!(target.connection_id, self.connection_id);
            assert_eq!(target.uri, "ui://documents/panel");
        }
    }

    #[async_trait]
    impl McpCallContextProvider for ResourceFallback {
        async fn prepare(
            &self,
            _: McpCallTarget<'_>,
            _: ToolCallView<'_>,
            _: &ToolDispatchContext,
        ) -> Result<Option<McpCallContext>, McpCallContextError> {
            panic!("resource reads must not borrow tool preparation")
        }

        async fn prepare_resource(
            &self,
            target: McpResourceTarget<'_>,
            context: &ToolDispatchContext,
        ) -> Result<Option<McpCallContext>, McpCallContextError> {
            self.verify_destination(&target);
            assert_eq!(std::ptr::from_ref(context) as usize, self.context_address);
            self.preparations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(McpCallContext::new(Default::default(), ())))
        }

        fn resource_authorization_target(
            &self,
            target: McpResourceTarget<'_>,
        ) -> Option<meerkat_core::authorization::OwnerQualifiedTarget> {
            self.verify_destination(&target);
            self.authorizations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(meerkat_core::authorization::OwnerQualifiedTarget {
                authority: meerkat_core::auth::PrincipalRef::in_domain(
                    meerkat_core::auth::PrincipalKind::ServiceAccount,
                    "resource-owner",
                    meerkat_core::auth::TrustDomainId::new("fixture-domain").unwrap(),
                )
                .unwrap(),
                namespace: "fixture-resource".into(),
                id: "owner-selected-resource".into(),
            })
        }
    }

    #[tokio::test]
    async fn selected_document_resources_refuse_without_tool_or_fallback_preparation() {
        let connection = resource_connection("documents").await;
        let (public, runtime) = configs("secret");
        let binding = Arc::new(binding(public, runtime.clone()).unwrap());
        let context = ToolDispatchContext::default();
        let fallback = Arc::new(ResourceFallback {
            config: connection.config().clone(),
            connection_id: connection.connection_id(),
            context_address: std::ptr::from_ref(&context) as usize,
            preparations: Default::default(),
            authorizations: Default::default(),
        });
        let provider = DocumentMcpContextProvider {
            bindings: vec![binding.clone()],
            fallback: Some(fallback.clone()),
        };
        // Exact runtime, public marker, and same-name changed destination all
        // stay refused. None can acquire resource authority from the fallback.
        for config in [&runtime, binding.public_config(), connection.config()] {
            let target = || McpResourceTarget {
                config,
                connection_id: connection.connection_id(),
                uri: "ui://documents/panel",
            };
            assert!(matches!(
                provider.prepare_resource(target(), &context).await,
                Err(McpCallContextError::Unavailable)
            ));
            assert!(provider.resource_authorization_target(target()).is_none());
        }
        assert_eq!(
            fallback
                .preparations
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            fallback
                .authorizations
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        close_resource_connection(connection).await;
    }

    #[tokio::test]
    async fn unrelated_resource_destination_preserves_both_fallback_methods() {
        let connection = resource_connection("ordinary").await;
        let (public, runtime) = configs("secret");
        let context = ToolDispatchContext::default();
        let fallback = Arc::new(ResourceFallback {
            config: connection.config().clone(),
            connection_id: connection.connection_id(),
            context_address: std::ptr::from_ref(&context) as usize,
            preparations: Default::default(),
            authorizations: Default::default(),
        });
        let provider = DocumentMcpContextProvider {
            bindings: vec![Arc::new(binding(public, runtime).unwrap())],
            fallback: Some(fallback.clone()),
        };
        // A document-shaped URI on an unrelated physical destination belongs
        // to its fallback owner, not the registered document service.
        let target = || McpResourceTarget {
            config: connection.config(),
            connection_id: connection.connection_id(),
            uri: "ui://documents/panel",
        };
        assert!(
            provider
                .prepare_resource(target(), &context)
                .await
                .unwrap()
                .is_some()
        );
        let authority = provider.resource_authorization_target(target()).unwrap();
        assert_eq!(authority.authority.id.as_str(), "resource-owner");
        assert_eq!(authority.namespace.as_ref(), "fixture-resource");
        assert_eq!(authority.id.as_ref(), "owner-selected-resource");
        assert_eq!(
            fallback
                .preparations
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            fallback
                .authorizations
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        close_resource_connection(connection).await;
    }

    #[tokio::test]
    async fn unrelated_resource_without_fallback_keeps_no_metadata_and_no_authority() {
        let connection = resource_connection("ordinary").await;
        let (public, runtime) = configs("secret");
        let provider = DocumentMcpContextProvider {
            bindings: vec![Arc::new(binding(public, runtime).unwrap())],
            fallback: None,
        };
        let target = || McpResourceTarget {
            config: connection.config(),
            connection_id: connection.connection_id(),
            uri: "ui://documents/panel",
        };
        assert!(
            provider
                .prepare_resource(target(), &ToolDispatchContext::default())
                .await
                .unwrap()
                .is_none()
        );
        assert!(provider.resource_authorization_target(target()).is_none());
        close_resource_connection(connection).await;
    }

    #[tokio::test]
    async fn pre_build_resolves_after_the_user_hook_and_keeps_the_authored_descriptor() {
        use meerkat_core::service::{
            CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy, SessionBuildOptions,
        };
        let (public, runtime) = configs("secret");
        let binding = Arc::new(binding(public, runtime.clone()).unwrap());
        let authored = binding.public_config().clone();
        let user_descriptor = authored.clone();
        let user_hook: crate::mob_handle_runtime::PreBuildHook = Arc::new(move |request| {
            let descriptor = user_descriptor.clone();
            Box::pin(async move {
                request
                    .build
                    .get_or_insert_with(SessionBuildOptions::default)
                    .mcp_servers = vec![descriptor];
                Ok(())
            })
        });
        let hook = pre_build_hook(Some(user_hook), vec![binding]);
        let mut request = CreateSessionRequest {
            model: "fixture-model".into(),
            prompt: "fixture".into(),
            injected_context: vec![],
            system_prompt: Default::default(),
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        };
        hook(&mut request).await.unwrap();
        assert_eq!(request.build.unwrap().mcp_servers, vec![runtime]);
        assert!(!serde_json::to_string(&authored).unwrap().contains("secret"));
    }
}
