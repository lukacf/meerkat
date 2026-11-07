//! Declaration-only admission reaches the real gateway callback customizer.
use super::*;
use meerkat_mobkit::identity_first::{CallbackBridge, GatewayAgentCustomizer};
use serde_json::{Value, json};

#[derive(Clone)]
struct Bridge {
    tools: Vec<Value>,
    scopes: Arc<Mutex<Vec<String>>>,
    dispatched: Arc<Mutex<Vec<String>>>,
    revoke: Option<Arc<AtomicBool>>,
}
#[async_trait::async_trait]
impl CallbackBridge for Bridge {
    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "callback/agent_customizer/customize_build" => {
                self.scopes
                    .lock()
                    .unwrap()
                    .push(params["scope_id"].as_str().unwrap().into());
                let mut draft = params["draft"].clone();
                draft["external_tools"] = json!(self.tools);
                if let Some(current) = &self.revoke {
                    current.store(false, Ordering::SeqCst);
                }
                Ok(draft)
            }
            "callback/call_tool" => {
                let scope = params["scope_id"].as_str().ok_or("missing scope")?;
                if !self
                    .scopes
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|known| known == scope)
                {
                    return Err("unregistered callback".into());
                }
                self.dispatched.lock().unwrap().push(scope.into());
                Ok(json!({"content":"actual gateway callback"}))
            }
            _ => Err("unexpected callback".into()),
        }
    }
}
fn test_bridge() -> Bridge {
    let callback = class().callbacks.remove(0);
    Bridge {
        tools: vec![
            json!({"name": callback.name, "description":callback.description,
            "input_schema":callback.input_schema}),
        ],
        scopes: Default::default(),
        dispatched: Default::default(),
        revoke: None,
    }
}

#[tokio::test]
async fn declaration_binds_actual_gateway_tools_without_changing_grants_and_reconstructs() {
    let (_directory, policy, owner, rule) = fixture().await;
    let runtime = build_runtime().await;
    let id = Id::new("gateway-one").unwrap();
    let instance = instance(&rule.class);
    let admitted = policy
        .reserve_member_class(id.clone(), instance.clone())
        .await
        .unwrap();
    assert_eq!(admitted.class(), &rule.class);
    assert!(admitted.dispatcher().is_none());
    let initial = owner.read().await.unwrap().1;
    assert!(initial.admissions[0].installed_surface.is_none());
    assert!(initial.admissions[0].native_surface.is_none());
    assert!(
        !serde_json::to_value(&initial.admissions[0])
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("installed_surface")
    );
    let bridge = test_bridge();
    let customizer = policy
        .member_class_gateway_customizer(Arc::new(GatewayAgentCustomizer::new(bridge.clone())));
    for _ in 0..2 {
        let (context, spec, mut draft) = build_arguments_for(&runtime, "project-one");
        customizer
            .customize_build(&context, &spec, &mut draft)
            .await
            .unwrap();
        assert_eq!(
            draft.external_tools.len(),
            1,
            "gateway declaration is preserved"
        );
        let dispatcher = draft.local_external_tools.dispatcher().unwrap();
        let row = owner.read().await.unwrap().1;
        assert_eq!(
            row.current, initial.current,
            "callback observation approves no grants"
        );
        assert_eq!(row.admissions.len(), 1);
        assert_eq!(
            row.admissions[0].installed_surface.as_ref().unwrap(),
            &dispatcher
                .tools()
                .iter()
                .map(|tool| tool.identity())
                .collect::<Vec<_>>()
        );
        assert!(row.admissions[0].native_surface.is_none());
        assert_eq!(row.admissions[0].phase, MemberClassAdmissionPhase::Reserved);
        let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
        dispatcher
            .dispatch(meerkat_core::ToolCallView {
                id: "actual",
                name: "project_read",
                args: &args,
            })
            .await
            .unwrap();
    }
    let scopes = bridge.scopes.lock().unwrap().clone();
    assert_eq!(scopes.len(), 2);
    assert_ne!(
        scopes[0], scopes[1],
        "each construction registers its actual callback scope"
    );
    assert_eq!(*bridge.dispatched.lock().unwrap(), scopes);
    owner.recover().await.unwrap();
    let replay = policy
        .reserve_member_class(id.clone(), instance.clone())
        .await
        .unwrap();
    assert!(
        replay.dispatcher().is_none(),
        "reservation never returns another build's dispatcher"
    );
    assert_eq!(owner.read().await.unwrap().1.admissions.len(), 1);
    let mut wrong = instance.clone();
    wrong.member.context_generation += 1;
    assert!(
        policy
            .reserve_member_class(id.clone(), wrong)
            .await
            .is_err()
    );
    let mut changed = test_bridge();
    changed
        .tools
        .push(json!({"name":"extra", "description":"extra", "input_schema":{"type":"object"}}));
    let changed =
        policy.member_class_gateway_customizer(Arc::new(GatewayAgentCustomizer::new(changed)));
    let (context, spec, mut draft) = build_arguments_for(&runtime, "project-one");
    assert!(
        changed
            .customize_build(&context, &spec, &mut draft)
            .await
            .is_err()
    );
    replay.retire().await.unwrap();
    assert!(policy.reserve_member_class(id, instance).await.is_err());
    assert!(
        runtime.list_members().await.is_empty(),
        "customizer control spawned no member"
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn gateway_rejects_missing_duplicate_and_changed_callback_definitions_before_binding() {
    let (_directory, policy, owner, rule) = fixture().await;
    policy
        .reserve_member_class(Id::new("gateway-one").unwrap(), instance(&rule.class))
        .await
        .unwrap();
    let original = test_bridge().tools;
    let mut schema = original.clone();
    schema[0]["input_schema"] = json!({"type":"string"});
    let mut description = original.clone();
    description[0]["description"] = json!("changed");
    let duplicate = vec![original[0].clone(), original[0].clone()];
    for tools in [vec![], schema, description, duplicate] {
        let mut bridge = test_bridge();
        bridge.tools = tools;
        let customizer = policy
            .member_class_gateway_customizer(Arc::new(GatewayAgentCustomizer::new(bridge.clone())));
        let (context, spec, mut draft) = build_arguments("project-one");
        assert!(
            customizer
                .customize_build(&context, &spec, &mut draft)
                .await
                .is_err()
        );
        assert!(
            owner.read().await.unwrap().1.admissions[0]
                .installed_surface
                .is_none()
        );
        assert!(bridge.dispatched.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn gateway_revocation_refuses_binding_but_exact_retirement_does_not_reapprove() {
    let (_directory, policy, owner, rule) = fixture().await;
    let id = Id::new("gateway-one").unwrap();
    let instance = instance(&rule.class);
    policy
        .reserve_member_class(id.clone(), instance.clone())
        .await
        .unwrap();
    let mut bridge = test_bridge();
    bridge.revoke = Some(rule.member_current.clone());
    let customizer =
        policy.member_class_gateway_customizer(Arc::new(GatewayAgentCustomizer::new(bridge)));
    let (context, spec, mut draft) = build_arguments("project-one");
    assert!(
        customizer
            .customize_build(&context, &spec, &mut draft)
            .await
            .is_err()
    );
    assert!(
        owner.read().await.unwrap().1.admissions[0]
            .installed_surface
            .is_none()
    );
    assert!(
        policy
            .reserve_member_class(id.clone(), instance.clone())
            .await
            .is_err()
    );
    let mut wrong = instance.member.member.clone();
    wrong.role = "another-profile".into();
    assert!(policy.retire_member_class(&id, &wrong).await.is_err());
    assert_eq!(
        owner.read().await.unwrap().1.admissions[0].phase,
        MemberClassAdmissionPhase::Reserved
    );
    policy
        .retire_member_class(&id, &instance.member.member)
        .await
        .unwrap();
    policy
        .retire_member_class(&id, &instance.member.member)
        .await
        .unwrap();
    assert_eq!(
        owner.read().await.unwrap().1.admissions[0].phase,
        MemberClassAdmissionPhase::Retired
    );
}
