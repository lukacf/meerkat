use super::*;
use meerkat_core::{
    BoundToolConsequencePolicy, ExecutionPolicyGatedDispatcher, MobMemberBinding, PolicyDigest,
    PolicyEvaluationProvenance, PolicyEvaluationSupervisorConfig, PolicyId,
    PolicyProviderGeneration, PolicyProviderId, PolicyRevision, ToolAccessPolicy,
    ToolConsequenceFailure, ToolConsequenceNarrowingPolicy, ToolConsequencePolicyRegistry,
    ToolConsequencePolicySnapshot, ToolConsequenceRequest, ToolConsequenceVerdict,
    ToolExecutionPolicy,
};

struct Barrier {
    entered: Semaphore,
    released: std::sync::Mutex<bool>,
    wake: std::sync::Condvar,
}
impl Barrier {
    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }
}
impl ToolConsequencePolicySnapshot for Barrier {
    fn provenance(&self) -> PolicyEvaluationProvenance {
        PolicyEvaluationProvenance {
            revision: PolicyRevision(1),
            digest: PolicyDigest::from_canonical_bytes(b"mcp-reload-barrier"),
        }
    }
    fn evaluate(&self, _: &ToolConsequenceRequest) -> ToolConsequenceVerdict {
        self.entered.add_permits(1);
        let _released = self
            .wake
            .wait_timeout_while(self.released.lock().unwrap(), LIMIT, |released| !*released)
            .unwrap();
        ToolConsequenceVerdict::Allow
    }
}
struct BarrierProvider {
    id: PolicyProviderId,
    barrier: Arc<Barrier>,
}
impl ToolConsequenceNarrowingPolicy for BarrierProvider {
    fn provider_id(&self) -> &PolicyProviderId {
        &self.id
    }
    fn generation(&self) -> PolicyProviderGeneration {
        PolicyProviderGeneration(1)
    }
    fn snapshot(
        &self,
        _: &PolicyId,
    ) -> Result<Arc<dyn ToolConsequencePolicySnapshot>, ToolConsequenceFailure> {
        Ok(self.barrier.clone())
    }
}
fn barrier_policy() -> (Arc<Barrier>, BoundToolConsequencePolicy) {
    let barrier = Arc::new(Barrier {
        entered: Semaphore::new(0),
        released: std::sync::Mutex::new(false),
        wake: std::sync::Condvar::new(),
    });
    let id = PolicyProviderId::new("mcp-reload-barrier").unwrap();
    let registry = ToolConsequencePolicyRegistry::new(
        vec![Arc::new(BarrierProvider {
            id: id.clone(),
            barrier: barrier.clone(),
        })],
        PolicyEvaluationSupervisorConfig {
            workers_per_provider: 1,
            queue_capacity_per_provider: 1,
            evaluation_deadline: LIMIT,
        },
        None,
    )
    .unwrap();
    let registry = Arc::new(registry);
    let policy = registry
        .bind(
            MobMemberBinding {
                mob_id: "fixture".into(),
                role: "reader".into(),
                member: "reader".into(),
            },
            id,
            PolicyId::new("read-only").unwrap(),
        )
        .unwrap();
    (barrier, policy)
}

#[tokio::test]
async fn read_only_plain_and_context_dispatch_refuse_reloaded_mutating_or_unknown_destination() {
    for with_context in [false, true] {
        for new_class in [ToolMutationClass::Mutating, ToolMutationClass::Unknown] {
            let fixture = Fixture::start().await;
            let mut config = fixture.config.clone();
            config
                .tool_names
                .insert("read".into(), "workspace_read".into());
            let mut source = Provider::new(config.clone());
            source.unselected_class = new_class;
            let provider = Arc::new(source);
            let adapter = Arc::new(McpRouterAdapter::new(
                fixture.router(Some(provider.clone()), config.clone()).await,
            ));
            let (barrier, policy) = barrier_policy();
            let gate = ExecutionPolicyGatedDispatcher::new(
                adapter.clone(),
                ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly).unwrap(),
            )
            .with_consequence_policy(policy);
            let result = AssertUnwindSafe(async {
                let args = serde_json::value::to_raw_value(&json!({})).unwrap();
                let call = ToolCallView {
                    id: "reload-race",
                    name: "workspace_read",
                    args: &args,
                };
                let current = context();
                let dispatch = async {
                    if with_context {
                        gate.dispatch_with_context(call, &current).await
                    } else {
                        gate.dispatch(call).await
                    }
                };
                let mut dispatch = Box::pin(dispatch);
                tokio::select! {
                    _ = &mut dispatch => panic!("dispatch skipped the consequence barrier"),
                    permit = barrier.entered.acquire() => permit.unwrap().forget(),
                    _ = tokio::time::sleep(LIMIT) => panic!("consequence policy never entered"),
                }
                if let McpTransportConfig::Http(http) = &mut config.transport {
                    http.headers
                        .insert("X-Changed-Destination".into(), "yes".into());
                }
                adapter.stage_reload(config).await.unwrap();
                adapter.apply_staged().await.unwrap();
                adapter.wait_until_ready(LIMIT).await.unwrap();
                assert_eq!(adapter.tool_mutation_class("workspace_read"), new_class);
                barrier.release();
                let error = tokio::time::timeout(LIMIT, dispatch)
                    .await
                    .unwrap()
                    .unwrap_err();
                assert_eq!(error, ToolError::access_denied("workspace_read"));
                assert_eq!(provider.prepared.load(Ordering::SeqCst), 0);
                assert!(fixture.server.requests.lock().unwrap().is_empty());
                assert!(
                    adapter
                        .external_tool_surface_snapshot()
                        .unwrap()
                        .entries
                        .iter()
                        .all(|entry| entry.inflight_call_count == 0)
                );
            })
            .catch_unwind()
            .await;
            barrier.release();
            drop(gate);
            Arc::try_unwrap(adapter).ok().unwrap().shutdown().await;
            fixture.finish(result).await;
        }
    }
}

#[tokio::test]
async fn nested_unrestricted_gate_preserves_read_only_restriction_and_actual_context() {
    let fixture = Fixture::start().await;
    let provider = Arc::new(Provider::new(fixture.config.clone()));
    let adapter = Arc::new(McpRouterAdapter::new(
        fixture
            .router(Some(provider.clone()), fixture.config.clone())
            .await,
    ));
    let inner = Arc::new(ExecutionPolicyGatedDispatcher::new(
        adapter.clone(),
        ToolExecutionPolicy::unrestricted(),
    ));
    let outer = ExecutionPolicyGatedDispatcher::new(
        inner.clone(),
        ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly).unwrap(),
    );
    let result = AssertUnwindSafe(async {
        let args = serde_json::value::to_raw_value(&json!({})).unwrap();
        let call = ToolCallView {
            id: "nested-read",
            name: "read",
            args: &args,
        };
        let current = context().with_turn_metadata(std::collections::BTreeMap::from([(
            "trace".into(),
            json!("preserved"),
        )]));
        outer.dispatch_with_context(call, &current).await.unwrap();
        assert_eq!(provider.restrictions.lock().unwrap().as_slice(), &[true]);
        assert_eq!(
            provider.observed.lock().unwrap()[0].0,
            *current.origin_session_id().unwrap()
        );
        assert!(!current.read_only_execution_required());
        inner.dispatch_with_context(call, &current).await.unwrap();
        assert_eq!(
            provider.restrictions.lock().unwrap().as_slice(),
            &[true, false]
        );
    })
    .catch_unwind()
    .await;
    drop(outer);
    drop(inner);
    Arc::try_unwrap(adapter).ok().unwrap().shutdown().await;
    fixture.finish(result).await;
}
