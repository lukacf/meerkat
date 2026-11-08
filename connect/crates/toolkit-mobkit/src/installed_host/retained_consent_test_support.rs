//! Small construction/readback seam for downstream durable consent controls.
//! Passive reads, stopped fixture provisioning and active cold restoration
//! are separate operations. Authority remains with their actual native owners.
use super::*;
use crate::private_originals::PrivateOriginalRef;
use std::sync::Weak;
use toolkit_store::{StoredRecord, ToolkitStore};

/// The caller supplies the real private connection's closed owner and retains
/// its normal bootstrap/shutdown protocol. Runtime/identity/registration and
/// controller binding remain the existing ToolkitGatewayHost hooks.
pub struct RetainedConsentControl {
    pub lifetime: Arc<NativeHostLifetime>,
    pub gateway: ToolkitGatewayHost,
    pub readback: RetainedConsentReadback,
}

impl RetainedConsentControl {
    /// Open an already provisioned, exact selected owner pair. Restore failure
    /// never initializes replacement stores. The optional trusted review floor
    /// is the existing public installed policy selection, not native permission.
    pub async fn open(
        selection: HostBootstrapSelection,
        closed: PrivateHostClosed,
        review: Option<Arc<dyn InstalledOperationReviewPolicy>>,
    ) -> Result<Self, HostLifetimeError> {
        let (lifetime, installed) =
            InstalledToolkitHost::open(selection, NativeOwnerStartup::Restore).await?;
        let lifetime = Arc::new(lifetime);
        let installed = installed.with_operation_review_policy(review);
        let readback = RetainedConsentReadback {
            lifetime: Arc::downgrade(&lifetime),
            source: Arc::downgrade(&installed.originals),
            store: Arc::downgrade(&installed.store),
            scope: installed.source_scope.clone(),
        };
        let gateway = installed.compose(lifetime.clone(), closed);
        Ok(Self {
            lifetime,
            gateway,
            readback,
        })
    }
}

/// Passive observation only. Weak owner references cannot keep an old installed
/// pair alive across the fixture's actual process-loss or joined-close boundary.
pub struct RetainedConsentReadback {
    lifetime: Weak<NativeHostLifetime>,
    source: Weak<PrivateOriginals>,
    store: Weak<SqliteStore>,
    scope: StoreScope,
}

pub struct RetainedConsentSourceRow {
    pub original: PrivateOriginalRef,
    pub record: StoredRecord,
}

impl RetainedConsentReadback {
    /// Read exactly three distinct canonical source records selected by their
    /// real retained references. This is not native admission, resumed custody,
    /// a completion receipt, or permission to replay an input.
    pub async fn snapshot(
        &self,
        originals: &[PrivateOriginalRef],
    ) -> Result<Vec<RetainedConsentSourceRow>, String> {
        if originals.len() != 3
            || originals
                .iter()
                .enumerate()
                .any(|(index, original)| originals[..index].contains(original))
        {
            return Err("three distinct retained source references are required".into());
        }
        let lifetime = self.lifetime.upgrade().ok_or("installed lifetime ended")?;
        lifetime
            .validate_current_selection()
            .map_err(|e| e.to_string())?;
        let source = self.source.upgrade().ok_or("installed source ended")?;
        let store = self.store.upgrade().ok_or("installed source store ended")?;
        let mut output = Vec::new();
        let mut keys = Vec::new();
        for original in originals {
            // Existing source decoding validates the exact reference and key;
            // the fixture does not reproduce the private record-key algorithm.
            let key = source
                .source_record_key(original)
                .await
                .map_err(|e| e.to_string())?;
            if key.realm != self.scope.realm || key.owner != self.scope.owner || keys.contains(&key)
            {
                return Err("retained source scope or distinctness mismatch".into());
            }
            let record = store
                .get(&key)
                .await
                .map_err(|e| e.to_string())?
                .ok_or("retained source disappeared")?;
            if source
                .source_record_key(original)
                .await
                .map_err(|e| e.to_string())?
                != key
                || store.get(&key).await.map_err(|e| e.to_string())?.as_ref() != Some(&record)
            {
                return Err("retained source changed during readback".into());
            }
            keys.push(key);
            output.push(RetainedConsentSourceRow {
                original: original.clone(),
                record,
            });
        }
        lifetime
            .validate_current_selection()
            .map_err(|e| e.to_string())?;
        Ok(output)
    }
}

/// Complete the existing inherited-pipe bootstrap with the exact installed
/// pair. The connection creates and owns its real closed observer.
pub async fn bootstrap(
    startup: crate::private_host::PrivateHostStartup,
    review: Arc<dyn InstalledOperationReviewPolicy>,
) -> Result<
    (
        RetainedConsentControl,
        crate::private_host::PrivateHostConnection,
    ),
    String,
> {
    let pending = Arc::new(std::sync::Mutex::new(None));
    let opened = pending.clone();
    let finish = pending.clone();
    let (control, connection, local, remote) =
        crate::private_host::PrivateHostConnection::bootstrap_prepared_with_controller_reviews(
            startup,
            move |selection| async move {
                let (lifetime, installed) =
                    InstalledToolkitHost::open(selection, NativeOwnerStartup::Restore).await?;
                let installed = installed.with_operation_review_policy(Some(review));
                let originals = installed.originals.clone();
                *opened.lock().map_err(|_| HostLifetimeError::Unavailable)? = Some(installed);
                Ok((lifetime, originals))
            },
            move |lifetime, closed, _logins| async move {
                let installed = finish
                    .lock()
                    .map_err(|_| HostLifetimeError::Unavailable)?
                    .take()
                    .ok_or(HostLifetimeError::Unavailable)?;
                let readback = RetainedConsentReadback {
                    lifetime: Arc::downgrade(&lifetime),
                    source: Arc::downgrade(&installed.originals),
                    store: Arc::downgrade(&installed.store),
                    scope: installed.source_scope.clone(),
                };
                let gateway = installed.compose(lifetime.clone(), closed);
                Ok(RetainedConsentControl {
                    lifetime,
                    gateway,
                    readback,
                })
            },
        )
        .await
        .map_err(|e| e.to_string())?;
    // This control issues no operator-review requests on the private channel.
    // Dropping these receivers refuses such requests through the normal owner.
    drop((local, remote));
    Ok((control, connection))
}

/// Select the fixed local fixture's exact existing operation tuple. The actual
/// native policy has already checked the grants before calling this floor.
pub struct RetainedConsentReview;
impl InstalledOperationReviewPolicy for RetainedConsentReview {
    fn review_floor(
        &self,
        context: super::InstalledOperationReviewContext<'_>,
    ) -> Result<
        meerkat_core::authorization::OperationReviewTier,
        meerkat_core::OperationAuthorizationError,
    > {
        match context.target() {
            meerkat_core::ToolOperationTarget::McpServerOperation {
                server,
                raw_operation,
                ..
            } if server == "toolkit-setup" && raw_operation == "setup_help" => {
                Ok(meerkat_core::authorization::OperationReviewTier::R3)
            }
            _ => Err(meerkat_core::OperationAuthorizationError::Unavailable),
        }
    }
}

/// The test supplies its actual factory-prepared surface and retained client.
/// This creates a new isolated installed bundle and provisions real native
/// roots while stopped. It cannot restore, overwrite or widen another bundle.
/// No source event, session, original admission or approval is created here.
pub fn provision(
    root: &Path,
    member: meerkat_core::MobMemberBinding,
    surface: &meerkat_core::PreparedToolSurface,
    controller: &meerkat_core::ControllerModelClient,
) -> Result<HostBootstrapSelection, String> {
    use meerkat_authorization_contracts::{
        constraints::AudienceRef,
        evidence::EvidenceId,
        work_association::{GrantTarget, TargetGrantLineage},
    };
    use meerkat_core::{
        CompiledApplicationToolPolicy, CompiledMemberToolAction, CompiledMemberToolGrant,
        CompiledMemberToolGrants, CompiledPolicySourceProvenance, CompiledToolConsequence,
        PolicyDigest, PolicyId, PolicyProviderId, PolicyRevision, PrincipalKind, PrincipalRef,
        TrustDomainId,
    };
    use meerkat_toolkit::host_lifetime::policy::{
        CompiledLocalPolicy, CompiledNativeSourceOrigin, CompiledNativeSourceRoute,
    };
    let failed = |e: HostLifetimeError| e.to_string();
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if root.join("current").exists() || root.join("shared").exists() {
        return Err("durable fixture provisioning requires an unused root".into());
    }
    let principal = |name| {
        PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            name,
            TrustDomainId::new("connect-durable-consent-fixture").unwrap(),
        )
        .unwrap()
    };
    let bundle = root.join("shared/policy/bundle.1");
    for path in [
        &bundle,
        &root.join("shared/data"),
        &root.join("shared/control"),
        &root.join("shared/activations"),
    ] {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|e| e.to_string())?;
    }
    let artifact = |name: &str, bytes: &[u8]| -> Result<CompiledArtifactReference, String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(bundle.join(name))
            .map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        Ok(CompiledArtifactReference {
            name: name.into(),
            sha256: digest(bytes),
        })
    };
    let source = artifact(
        "policy.json",
        b"{\"schema\":1,\"purpose\":\"durable-consent-census\"}",
    )?;
    let tool_policy = CompiledApplicationToolPolicy::new(
        PolicyProviderId::new("durable-consent").unwrap(),
        PolicyId::new("exact-mcp").unwrap(),
        PolicyRevision(1),
        CompiledPolicySourceProvenance {
            source_id: "durable-consent-fixture".into(),
            source_digest: PolicyDigest::from_canonical_bytes(b"durable-consent-fixture"),
        },
        vec![CompiledMemberToolGrants {
            member_identity: member.member.clone(),
            grants: vec![CompiledMemberToolGrant {
                tool_name: "setup_help".into(),
                action: CompiledMemberToolAction::Invoke,
                consequence: CompiledToolConsequence::R3,
            }],
        }],
    )
    .map_err(|e| e.to_string())?;
    let tool = artifact(
        "application-tool-policy.json",
        &tool_policy.canonical_json().map_err(|e| e.to_string())?,
    )?;
    let initial = artifact(
        "member-surface.json",
        &canonical_json(&surface.tools).map_err(|e| e.to_string())?,
    )?;
    let application = principal("application");
    let operator = principal("operator");
    let executor = principal("member");
    let audience = AudienceRef::Principal {
        principal: operator.clone(),
    };
    let mut local = CompiledLocalPolicy {
        schema: 7,
        deployment: toolkit_package::Id::new("durable-consent").unwrap(),
        source_policy_sha256: source.sha256.clone(),
        components: vec![],
        channels: vec![],
        native_host: Some(CompiledNativeHost {
            setup: None,
            service_parent_administration: None,
            authentication_domain: None,
            event_domain: None,
            remote_operator: Some(operator.clone()),
            realm: meerkat_core::mob_realm_id(&member.mob_id).map_err(|e| e.to_string())?,
            grant_administrator: application.clone(),
            ingress_domain: ResourceDomain {
                authority: principal("source"),
                namespace: "events".into(),
            },
            application_tool_policy: tool.clone(),
            member_classes: vec![],
            members: vec![CompiledNativeMember {
                member: member.clone(),
                logical_owner: executor.clone(),
                context_generation: 1,
                audience: audience.clone(),
                initial_surface: initial.clone(),
                setup_capability: None,
            }],
            routes: vec![CompiledNativeSourceRoute {
                id: toolkit_package::Id::new("console").unwrap(),
                application_parent: None,
                controller_parent: None,
                origin: CompiledNativeSourceOrigin::LocalConsole {
                    member: member.clone(),
                    requester: operator.clone(),
                    ingress_actor: principal("console"),
                },
                logical_executor: executor.clone(),
                represented_subject: None,
                source_authority: principal("console"),
                authentication_domain: ResourceDomain {
                    authority: principal("console"),
                    namespace: "login".into(),
                },
                event_domain: ResourceDomain {
                    authority: principal("console"),
                    namespace: "input".into(),
                },
            }],
        }),
    };
    let local_ref = artifact(
        "toolkit-local-policy.json",
        &canonical_json(&local).map_err(|e| e.to_string())?,
    )?;
    let manifest = artifact("manifest.json", &canonical_json(&serde_json::json!({"artifacts":
        ([source, tool, initial, local_ref.clone()].iter().map(|r| serde_json::json!({"path":r.name,"sha256":r.sha256})).collect::<Vec<_>>())
    })).map_err(|e| e.to_string())?)?;
    let activation_path = root.join("shared/activations/1.json");
    let activation = canonical_json(&serde_json::json!({"activation_id":"1","policy_bundle":{
        "path":"shared/policy/bundle.1","policy_digest":format!("policy-v1:sha256:{}",manifest.sha256)}})).map_err(|e| e.to_string())?;
    fs::write(&activation_path, &activation).map_err(|e| e.to_string())?;
    std::os::unix::fs::symlink("shared/activations/1.json", root.join("current"))
        .map_err(|e| e.to_string())?;
    let mut selected = HostBootstrapSelection {
        deployment_root: root,
        activation: meerkat_toolkit::host_lifetime::control::SelectedHostArtifact {
            path: activation_path,
            sha256: digest(&activation),
        },
        policy_manifest: meerkat_toolkit::host_lifetime::control::SelectedHostArtifact {
            path: bundle.join("manifest.json"),
            sha256: manifest.sha256,
        },
        local_policy: meerkat_toolkit::host_lifetime::control::SelectedHostArtifact {
            path: bundle.join("toolkit-local-policy.json"),
            sha256: local_ref.sha256,
        },
    };
    operator_setup::OperatorSetup::provision_application_candidate(
        &selected,
        "durable-consent-grants",
        1,
    )
    .map_err(failed)?;
    let facts = controller.plain_facts().map_err(|e| e.to_string())?;
    let helper = NativeHostLifetime::with_candidate_installation(&selected, |policy| {
        let native = policy
            .native_host
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        let directory = state_directory(&selected, native, NativeOwnerStartup::Restore)?;
        let (source, store) = operator_setup::open_owner_pair_in(
            &directory,
            &native.ingress_domain,
            Some(&operator),
            operator_setup::OperatorOwnerStartup::Restore,
            false,
        )?;
        let grants = source.grants();
        let parent = grants
            .issue_root(
                &meerkat_core::AdmissionGuard::none(),
                &operator,
                EvidenceId::new("controller-parent").unwrap(),
                application.clone(),
                None,
                application_operations::parent_restrictions(application_operations::controller(
                    &application,
                    &executor,
                    &audience,
                    &facts.route_key(),
                    application_operations::ControllerAction::Inference,
                )),
            )
            .map_err(|_| HostLifetimeError::Denied)?;
        let operation = grants
            .issue_root(
                &meerkat_core::AdmissionGuard::none(),
                &operator,
                EvidenceId::new("mcp-selection").unwrap(),
                executor.clone(),
                None,
                application_operations::mcp_access_restrictions(
                    &application,
                    &executor,
                    &audience,
                    "toolkit-setup",
                    ["setup_help"],
                    &meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
                ),
            )
            .map_err(|_| HostLifetimeError::Denied)?;
        let helper = acquisition::InstalledSetupSelection {
            catalog_capability: None,
            report_capability: None,
            member: member.clone(),
            ingress_actor: application.clone(),
            logical_executor: executor.clone(),
            represented_subject: None,
            controller_parent: vec![parent],
            operation_basis: WorkAuthorityBasis::TargetedGrantLineages {
                base: None,
                targets: vec![TargetGrantLineage {
                    target: GrantTarget::McpServer {
                        server: "toolkit-setup".into(),
                    },
                    lineage: vec![operation],
                }],
            },
        };
        drop(store);
        drop(source);
        Ok(helper)
    })
    .map_err(failed)?;
    let request = operator_setup::AgentSetupCapabilityRequest {
        member: member.clone(),
        route: toolkit_package::Id::new("console").unwrap(),
        issuance_id: EvidenceId::new("caller-find-parent").unwrap(),
        helper: Some(member),
        helper_targets: vec![operator_setup::SetupTargetRequest {
            target: Some(GrantTarget::McpServer {
                server: "toolkit-setup".into(),
            }),
            issuance_id: EvidenceId::new("caller-mcp-parent").unwrap(),
        }],
    };
    let capability = operator_setup::OperatorSetup::configure_application_agent_setup_with_helper(
        &selected,
        &request,
        |_| Ok(Some(helper.clone())),
    )
    .map_err(failed)?;
    let native = local.native_host.as_mut().unwrap();
    native.routes[0].controller_parent = Some(helper.controller_parent);
    native.members[0].setup_capability = Some(capability);
    // The test owns this stopped bundle. Update its actual manifest and
    // activation bindings before opening any runtime or source admission.
    let bytes = canonical_json(&local).map_err(|e| e.to_string())?;
    fs::write(&selected.local_policy.path, &bytes).map_err(|e| e.to_string())?;
    selected.local_policy.sha256 = digest(&bytes);
    let mut manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(&selected.policy_manifest.path).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    for row in manifest["artifacts"]
        .as_array_mut()
        .ok_or("invalid fixture manifest")?
    {
        if row["path"] == "toolkit-local-policy.json" {
            row["sha256"] = selected.local_policy.sha256.clone().into();
        }
    }
    let bytes = canonical_json(&manifest).map_err(|e| e.to_string())?;
    fs::write(&selected.policy_manifest.path, &bytes).map_err(|e| e.to_string())?;
    selected.policy_manifest.sha256 = digest(&bytes);
    let mut activation: serde_json::Value =
        serde_json::from_slice(&activation).map_err(|e| e.to_string())?;
    activation["policy_bundle"]["policy_digest"] =
        format!("policy-v1:sha256:{}", selected.policy_manifest.sha256).into();
    let bytes = canonical_json(&activation).map_err(|e| e.to_string())?;
    fs::write(&selected.activation.path, &bytes).map_err(|e| e.to_string())?;
    selected.activation.sha256 = digest(&bytes);
    Ok(selected)
}

impl RetainedConsentControl {
    /// A separate active operation, unlike snapshot(): obtain actual native
    /// preparation, then invoke the existing canonical cold recovery owner.
    /// No BoundGovernedInput is returned, so no replacement input is submitted.
    pub async fn restore_promoted(
        &self,
        submission: crate::private_originals::PrivateOriginalSubmit,
    ) -> Result<(), String> {
        self.lifetime
            .validate_current_selection()
            .map_err(|e| e.to_string())?;
        self.readback
            .source
            .upgrade()
            .ok_or("installed source ended")?
            .restore_consent_original_for_test(submission)
            .await
            .map_err(|e| e.to_string())
    }
}
