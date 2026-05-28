use meerkat_machine_dsl::machine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionDurableProviderKind {
    Anthropic,
    OpenAI,
    Gemini,
    SelfHosted,
    #[default]
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionToolCategoryOverrideKind {
    #[default]
    Inherit,
    Enable,
    Disable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionCallTimeoutOverrideKind {
    #[default]
    Inherit,
    Disabled,
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum SessionSystemPromptSource {
    #[default]
    DirectMutation,
    ExplicitBuild,
    DefaultBuild,
    WasmDefaultBuild,
    RuntimeContextAppend,
    RuntimeSteerCleanup,
}

machine! {
    machine SessionDurableConfigAuthorityMachine {
        version: 1,
        rust: "self" / "catalog::dsl::session_durable_config_authority",

        state {
            lifecycle_phase: SessionDurableConfigAuthorityPhase,
        }

        init(Ready) {}

        terminal []

        phase SessionDurableConfigAuthorityPhase {
            Ready,
        }

        input SessionDurableConfigAuthorityInput {
            AuthorizeSessionMetadataPersist {
                schema_version: u64,
                model_present: bool,
                max_tokens: u64,
                structured_output_retries: u64,
                provider: Enum<SessionDurableProviderKind>,
                self_hosted_server_present: bool,
                provider_params_present: bool,
                tooling_builtins: Enum<SessionToolCategoryOverrideKind>,
                tooling_shell: Enum<SessionToolCategoryOverrideKind>,
                tooling_comms: Enum<SessionToolCategoryOverrideKind>,
                tooling_mob: Enum<SessionToolCategoryOverrideKind>,
                tooling_memory: Enum<SessionToolCategoryOverrideKind>,
                tooling_schedule: Enum<SessionToolCategoryOverrideKind>,
                tooling_workgraph: Enum<SessionToolCategoryOverrideKind>,
                tooling_image_generation: Enum<SessionToolCategoryOverrideKind>,
                tooling_web_search: Enum<SessionToolCategoryOverrideKind>,
                active_skill_count: u64,
                keep_alive: bool,
                comms_name_present: bool,
                peer_meta_present: bool,
                realm_id_present: bool,
                instance_id_present: bool,
                backend_present: bool,
                config_generation_present: bool,
                auth_binding_present: bool,
            },
            AuthorizeSessionBuildStatePersist {
                system_prompt_present: bool,
                output_schema_present: bool,
                hook_entry_count: u64,
                disabled_hook_count: u64,
                budget_limits_present: bool,
                recoverable_tool_count: u64,
                silent_comms_intent_count: u64,
                max_inline_peer_notifications_present: bool,
                app_context_present: bool,
                additional_instruction_count: u64,
                shell_env_count: u64,
                mob_tool_authority_context_present: bool,
                mob_tool_authority_context_generated: bool,
                call_timeout_override: Enum<SessionCallTimeoutOverrideKind>,
            },
            RestoreSessionBuildState {
                system_prompt_present: bool,
                output_schema_present: bool,
                hook_entry_count: u64,
                disabled_hook_count: u64,
                budget_limits_present: bool,
                recoverable_tool_count: u64,
                silent_comms_intent_count: u64,
                max_inline_peer_notifications_present: bool,
                app_context_present: bool,
                additional_instruction_count: u64,
                shell_env_count: u64,
                mob_tool_authority_context_present: bool,
                call_timeout_override: Enum<SessionCallTimeoutOverrideKind>,
            },
            AuthorizeSystemPromptMutation {
                source: Enum<SessionSystemPromptSource>,
                prompt_present: bool,
                prompt_byte_count: u64,
                replacing_existing: bool,
            },
        }

        effect SessionDurableConfigAuthorityEffect {
            SessionMetadataPersistAuthorized {
                schema_version: u64,
                model_present: bool,
                max_tokens: u64,
                structured_output_retries: u64,
                provider: Enum<SessionDurableProviderKind>,
                self_hosted_server_present: bool,
                provider_params_present: bool,
                tooling_builtins: Enum<SessionToolCategoryOverrideKind>,
                tooling_shell: Enum<SessionToolCategoryOverrideKind>,
                tooling_comms: Enum<SessionToolCategoryOverrideKind>,
                tooling_mob: Enum<SessionToolCategoryOverrideKind>,
                tooling_memory: Enum<SessionToolCategoryOverrideKind>,
                tooling_schedule: Enum<SessionToolCategoryOverrideKind>,
                tooling_workgraph: Enum<SessionToolCategoryOverrideKind>,
                tooling_image_generation: Enum<SessionToolCategoryOverrideKind>,
                tooling_web_search: Enum<SessionToolCategoryOverrideKind>,
                active_skill_count: u64,
                keep_alive: bool,
                comms_name_present: bool,
                peer_meta_present: bool,
                realm_id_present: bool,
                instance_id_present: bool,
                backend_present: bool,
                config_generation_present: bool,
                auth_binding_present: bool,
            },
            SessionBuildStatePersistAuthorized {
                system_prompt_present: bool,
                output_schema_present: bool,
                hook_entry_count: u64,
                disabled_hook_count: u64,
                budget_limits_present: bool,
                recoverable_tool_count: u64,
                silent_comms_intent_count: u64,
                max_inline_peer_notifications_present: bool,
                app_context_present: bool,
                additional_instruction_count: u64,
                shell_env_count: u64,
                mob_tool_authority_context_present: bool,
                mob_tool_authority_context_generated: bool,
                call_timeout_override: Enum<SessionCallTimeoutOverrideKind>,
            },
            SessionBuildStateRestoreAuthorized {
                system_prompt_present: bool,
                output_schema_present: bool,
                hook_entry_count: u64,
                disabled_hook_count: u64,
                budget_limits_present: bool,
                recoverable_tool_count: u64,
                silent_comms_intent_count: u64,
                max_inline_peer_notifications_present: bool,
                app_context_present: bool,
                additional_instruction_count: u64,
                shell_env_count: u64,
                mob_tool_authority_context_present: bool,
                call_timeout_override: Enum<SessionCallTimeoutOverrideKind>,
            },
            SystemPromptMutationAuthorized {
                source: Enum<SessionSystemPromptSource>,
                prompt_present: bool,
                prompt_byte_count: u64,
                replacing_existing: bool,
            },
        }

        disposition SessionMetadataPersistAuthorized => local,
        disposition SessionBuildStatePersistAuthorized => local,
        disposition SessionBuildStateRestoreAuthorized => local,
        disposition SystemPromptMutationAuthorized => local,

        transition AuthorizeSessionMetadataPersist {
            on input AuthorizeSessionMetadataPersist {
                schema_version,
                model_present,
                max_tokens,
                structured_output_retries,
                provider,
                self_hosted_server_present,
                provider_params_present,
                tooling_builtins,
                tooling_shell,
                tooling_comms,
                tooling_mob,
                tooling_memory,
                tooling_schedule,
                tooling_workgraph,
                tooling_image_generation,
                tooling_web_search,
                active_skill_count,
                keep_alive,
                comms_name_present,
                peer_meta_present,
                realm_id_present,
                instance_id_present,
                backend_present,
                config_generation_present,
                auth_binding_present,
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && schema_version > 0
                && model_present == true
            }
            update {}
            to Ready
            emit SessionMetadataPersistAuthorized {
                schema_version: schema_version,
                model_present: model_present,
                max_tokens: max_tokens,
                structured_output_retries: structured_output_retries,
                provider: provider,
                self_hosted_server_present: self_hosted_server_present,
                provider_params_present: provider_params_present,
                tooling_builtins: tooling_builtins,
                tooling_shell: tooling_shell,
                tooling_comms: tooling_comms,
                tooling_mob: tooling_mob,
                tooling_memory: tooling_memory,
                tooling_schedule: tooling_schedule,
                tooling_workgraph: tooling_workgraph,
                tooling_image_generation: tooling_image_generation,
                tooling_web_search: tooling_web_search,
                active_skill_count: active_skill_count,
                keep_alive: keep_alive,
                comms_name_present: comms_name_present,
                peer_meta_present: peer_meta_present,
                realm_id_present: realm_id_present,
                instance_id_present: instance_id_present,
                backend_present: backend_present,
                config_generation_present: config_generation_present,
                auth_binding_present: auth_binding_present
            }
        }

        transition AuthorizeSessionBuildStatePersist {
            on input AuthorizeSessionBuildStatePersist {
                system_prompt_present,
                output_schema_present,
                hook_entry_count,
                disabled_hook_count,
                budget_limits_present,
                recoverable_tool_count,
                silent_comms_intent_count,
                max_inline_peer_notifications_present,
                app_context_present,
                additional_instruction_count,
                shell_env_count,
                mob_tool_authority_context_present,
                mob_tool_authority_context_generated,
                call_timeout_override,
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (
                    mob_tool_authority_context_present == false
                    || mob_tool_authority_context_generated == true
                )
            }
            update {}
            to Ready
            emit SessionBuildStatePersistAuthorized {
                system_prompt_present: system_prompt_present,
                output_schema_present: output_schema_present,
                hook_entry_count: hook_entry_count,
                disabled_hook_count: disabled_hook_count,
                budget_limits_present: budget_limits_present,
                recoverable_tool_count: recoverable_tool_count,
                silent_comms_intent_count: silent_comms_intent_count,
                max_inline_peer_notifications_present: max_inline_peer_notifications_present,
                app_context_present: app_context_present,
                additional_instruction_count: additional_instruction_count,
                shell_env_count: shell_env_count,
                mob_tool_authority_context_present: mob_tool_authority_context_present,
                mob_tool_authority_context_generated: mob_tool_authority_context_generated,
                call_timeout_override: call_timeout_override
            }
        }

        transition RestoreSessionBuildState {
            on input RestoreSessionBuildState {
                system_prompt_present,
                output_schema_present,
                hook_entry_count,
                disabled_hook_count,
                budget_limits_present,
                recoverable_tool_count,
                silent_comms_intent_count,
                max_inline_peer_notifications_present,
                app_context_present,
                additional_instruction_count,
                shell_env_count,
                mob_tool_authority_context_present,
                call_timeout_override,
            }
            guard { self.lifecycle_phase == Phase::Ready }
            update {}
            to Ready
            emit SessionBuildStateRestoreAuthorized {
                system_prompt_present: system_prompt_present,
                output_schema_present: output_schema_present,
                hook_entry_count: hook_entry_count,
                disabled_hook_count: disabled_hook_count,
                budget_limits_present: budget_limits_present,
                recoverable_tool_count: recoverable_tool_count,
                silent_comms_intent_count: silent_comms_intent_count,
                max_inline_peer_notifications_present: max_inline_peer_notifications_present,
                app_context_present: app_context_present,
                additional_instruction_count: additional_instruction_count,
                shell_env_count: shell_env_count,
                mob_tool_authority_context_present: mob_tool_authority_context_present,
                call_timeout_override: call_timeout_override
            }
        }

        transition AuthorizeSystemPromptMutation {
            on input AuthorizeSystemPromptMutation {
                source,
                prompt_present,
                prompt_byte_count,
                replacing_existing,
            }
            guard {
                self.lifecycle_phase == Phase::Ready
                && (prompt_present == true || prompt_byte_count == 0)
            }
            update {}
            to Ready
            emit SystemPromptMutationAuthorized {
                source: source,
                prompt_present: prompt_present,
                prompt_byte_count: prompt_byte_count,
                replacing_existing: replacing_existing
            }
        }
    }
}
