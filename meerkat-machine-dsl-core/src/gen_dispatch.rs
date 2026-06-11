use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::Ident;

use crate::ast::*;
use crate::gen_state::state_struct_name;

/// Controls how `self.field` references are generated.
#[derive(Clone, Copy)]
pub(crate) enum FieldPrefix {
    /// `self.state.field` — inside authority methods
    AuthorityState,
    /// `self.field` — inside state struct methods (e.g., phase())
    DirectSelf,
}

/// Generate the sealed dispatch function (apply/apply_signal).
pub fn generate(def: &MachineDef) -> TokenStream {
    let state_name = state_struct_name(def);
    let input_name = &def.inputs.name;
    let input_variant_name = format_ident!("{}Variant", input_name);
    let phase_name = &def.phase_enum.name;
    let effect_name = &def.effects.name;
    let machine_name = &def.name;

    let transition_name = format_ident!("{}Transition", machine_name);
    let error_name = format_ident!("{}TransitionError", machine_name);
    let trigger_name = format_ident!("{}TransitionTrigger", machine_name);
    let authority_name = format_ident!("{}Authority", machine_name);
    let authority_snapshot_name = format_ident!("{}AuthoritySnapshot", machine_name);
    let prepared_authority_name = format_ident!("{}PreparedAuthority", machine_name);
    let prepared_commit_error_name = format_ident!("{}PreparedCommitError", machine_name);
    let prepared_commit_effect_error_name =
        format_ident!("{}PreparedCommitEffectError", machine_name);
    let mutator_trait = format_ident!("{}Mutator", machine_name);

    let input_transitions: Vec<_> = def
        .transitions
        .iter()
        .filter(|t| matches!(t.trigger.kind, TriggerKindDef::Input))
        .collect();
    let signal_transitions: Vec<_> = def
        .transitions
        .iter()
        .filter(|t| matches!(t.trigger.kind, TriggerKindDef::Signal))
        .collect();

    let (input_arms, input_methods) = gen_input_match_arms_and_methods(
        def,
        &input_transitions,
        input_name,
        &error_name,
        &input_variant_name,
        effect_name,
        quote! { #trigger_name::Input },
    );

    let has_signals = !def.signals.variants.is_empty() && !signal_transitions.is_empty();
    let signal_name = &def.signals.name;
    let signal_variant_name = format_ident!("{}Variant", signal_name);
    let (signal_arms, signal_methods) = if has_signals {
        gen_signal_match_arms_and_methods(
            def,
            &signal_transitions,
            signal_name,
            &error_name,
            &signal_variant_name,
            effect_name,
            quote! { #trigger_name::Signal },
        )
    } else {
        (TokenStream::new(), TokenStream::new())
    };
    let signal_trigger_variant = if def.signals.variants.is_empty() {
        TokenStream::new()
    } else {
        quote! { Signal(#signal_variant_name), }
    };
    let signal_trigger_display = if def.signals.variants.is_empty() {
        TokenStream::new()
    } else {
        quote! {
            Self::Signal(variant) => write!(f, "signal::{variant}"),
        }
    };

    let invariant_checks: Vec<_> = def
        .invariants
        .iter()
        .map(|inv| {
            let inv_name_str = inv.name.to_string();
            let check = gen_expr(&inv.expr, FieldPrefix::AuthorityState);
            quote! {
                debug_assert!(#check, "invariant '{}' violated", #inv_name_str);
            }
        })
        .collect();
    let recovered_state_validations: Vec<_> = def
        .invariants
        .iter()
        .map(|inv| {
            let inv_name_str = inv.name.to_string();
            let check = gen_expr(&inv.expr, FieldPrefix::AuthorityState);
            quote! {
                if !(#check) {
                    return Err(#error_name::RecoveredStateInvariantRejected {
                        phase: self.state.phase(),
                        invariant: #inv_name_str,
                    });
                }
            }
        })
        .collect();

    let helpers: Vec<_> = def.helpers.iter().map(gen_helper).collect();

    let signal_method = if has_signals {
        quote! {
            pub fn apply_signal(&mut self, signal: #signal_name) -> Result<#transition_name, #error_name> {
                let from_phase = self.state.phase();
                let signal_variant = signal.variant();
                let mut effects = Vec::new();

                match signal {
                    #signal_arms
                    #[allow(unreachable_patterns)]
                    _ => return Err(#error_name::NoMatchingTransition {
                        phase: from_phase,
                        trigger: #trigger_name::Signal(signal_variant),
                    }),
                }

                let to_phase = self.state.phase();
                #(#invariant_checks)*
                Ok(#transition_name {
                    from_phase,
                    to_phase,
                    effects,
                    _origin: sealed::TransitionOrigin,
                })
            }
        }
    } else {
        TokenStream::new()
    };
    let prepared_signal_method = if has_signals {
        quote! {
            pub fn apply_signal(&mut self, signal: #signal_name) -> Result<#transition_name, #error_name> {
                self.authority.apply_signal(signal)
            }
        }
    } else {
        TokenStream::new()
    };

    quote! {
        // Type alias so DSL expressions can use `Phase::Variant`
        use #phase_name as Phase;

        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct #transition_name {
            pub from_phase: #phase_name,
            pub to_phase: #phase_name,
            effects: Vec<#effect_name>,
            _origin: sealed::TransitionOrigin,
        }

        impl #transition_name {
            pub fn effects(&self) -> &[#effect_name] {
                &self.effects
            }

            pub fn into_effects(self) -> Vec<#effect_name> {
                self.effects
            }
        }

        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum #error_name {
            /// No transition is declared for this `(phase, trigger)` pair at
            /// all — the trigger variant is semantically out of scope for
            /// the current phase. Shell callers should treat this as a hard
            /// error: firing the wrong input for the current phase is a
            /// programming mistake.
            NoMatchingTransition { phase: #phase_name, trigger: #trigger_name },
            /// A transition is declared for this `(phase, trigger)` pair
            /// but every candidate transition's guard(s) evaluated false.
            /// Typed signal that the input was *rejected on state*, not
            /// *unrecognised*. Shell callers that fire idempotently (e.g.,
            /// a realtime dispatcher firing `ProductTurnCommitted` on every
            /// observed `TurnCommitted` event, expecting the DSL to drop
            /// duplicates) should treat this as a successful no-op rather
            /// than an error.
            GuardRejected { phase: #phase_name, trigger: #trigger_name },
            /// A recovered authority state violated a generated invariant
            /// before any transition was attempted.
            RecoveredStateInvariantRejected { phase: #phase_name, invariant: &'static str },
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum #trigger_name {
            Input(#input_variant_name),
            #signal_trigger_variant
        }

        impl std::fmt::Display for #trigger_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::Input(variant) => write!(f, "input::{variant}"),
                    #signal_trigger_display
                }
            }
        }

        impl std::fmt::Display for #error_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::NoMatchingTransition { phase, trigger } => {
                        write!(f, "no matching transition from phase {:?} for {}", phase, trigger)
                    }
                    Self::GuardRejected { phase, trigger } => {
                        write!(f, "guard rejected transition from phase {:?} for {}", phase, trigger)
                    }
                    Self::RecoveredStateInvariantRejected { phase, invariant } => {
                        write!(
                            f,
                            "recovered authority state violated invariant '{}' in phase {:?}",
                            invariant,
                            phase
                        )
                    }
                }
            }
        }

        impl std::error::Error for #error_name {}

        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum #prepared_commit_error_name {
            /// A generated prepared-authority commit was attempted after the
            /// live authority moved away from the prepared base state.
            BaseChanged { current_phase: #phase_name, base_phase: #phase_name },
        }

        impl std::fmt::Display for #prepared_commit_error_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::BaseChanged { current_phase, base_phase } => {
                        write!(
                            f,
                            "prepared authority base changed before commit (base {:?}, current {:?})",
                            base_phase,
                            current_phase
                        )
                    }
                }
            }
        }

        impl std::error::Error for #prepared_commit_error_name {}

        #[derive(Debug)]
        pub enum #prepared_commit_effect_error_name<E> {
            Commit(#prepared_commit_error_name),
            Effect(E),
        }

        impl<E: std::fmt::Display> std::fmt::Display for #prepared_commit_effect_error_name<E> {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::Commit(error) => write!(f, "{error}"),
                    Self::Effect(error) => write!(f, "{error}"),
                }
            }
        }

        impl<E: std::error::Error + 'static> std::error::Error for #prepared_commit_effect_error_name<E> {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                match self {
                    Self::Commit(error) => Some(error),
                    Self::Effect(error) => Some(error),
                }
            }
        }

        mod sealed {
            #[derive(Debug, Clone, Copy, PartialEq, Eq)]
            pub struct TransitionOrigin;

            pub trait Sealed {}
        }

        pub trait #mutator_trait: sealed::Sealed {
            fn apply(&mut self, input: #input_name) -> Result<#transition_name, #error_name>;
        }

        pub struct #authority_name {
            state: #state_name,
            authority_owner_token: std::sync::Arc<dyn std::any::Any + Send + Sync>,
        }

        pub struct #prepared_authority_name {
            base_state: #state_name,
            authority: #authority_name,
        }

        #[derive(Clone, Debug)]
        pub struct #authority_snapshot_name {
            state: #state_name,
        }

        impl #authority_snapshot_name {
            pub fn state(&self) -> &#state_name {
                &self.state
            }
        }

        impl sealed::Sealed for #authority_name {}
        impl sealed::Sealed for #prepared_authority_name {}

        impl #prepared_authority_name {
            pub fn state(&self) -> &#state_name {
                self.authority.state()
            }

            #prepared_signal_method

            fn validate_commit_base(
                &self,
                live: &#authority_name,
            ) -> Result<(), #prepared_commit_error_name> {
                if live.state != self.base_state {
                    return Err(#prepared_commit_error_name::BaseChanged {
                        current_phase: live.state.phase(),
                        base_phase: self.base_state.phase(),
                    });
                }
                Ok(())
            }

            fn commit_into_unchecked(self, live: &mut #authority_name) {
                live.state = self.authority.state;
            }

            fn commit_into(
                self,
                live: &mut #authority_name,
            ) -> Result<(), #prepared_commit_error_name> {
                self.validate_commit_base(live)?;
                self.commit_into_unchecked(live);
                Ok(())
            }
        }

        impl #mutator_trait for #prepared_authority_name {
            fn apply(&mut self, input: #input_name) -> Result<#transition_name, #error_name> {
                self.authority.apply(input)
            }
        }

        impl #authority_name {
            pub fn new() -> Self {
                Self {
                    state: #state_name::default(),
                    authority_owner_token: std::sync::Arc::new(()),
                }
            }

            pub fn state(&self) -> &#state_name {
                &self.state
            }

            pub fn recover_from_state(state: #state_name) -> Result<Self, #error_name> {
                let authority = Self {
                    state,
                    authority_owner_token: std::sync::Arc::new(()),
                };
                authority.validate_recovered_state()?;
                Ok(authority)
            }

            fn validate_recovered_state(&self) -> Result<(), #error_name> {
                #(#recovered_state_validations)*
                Ok(())
            }

            pub fn fork(&self) -> Self {
                Self {
                    state: self.state.clone(),
                    authority_owner_token: std::sync::Arc::new(()),
                }
            }

            pub fn prepare_authority(&self) -> #prepared_authority_name {
                #prepared_authority_name {
                    base_state: self.state.clone(),
                    authority: self.fork(),
                }
            }

            pub fn commit_prepared_authority(
                &mut self,
                prepared: #prepared_authority_name,
            ) -> Result<(), #prepared_commit_error_name> {
                prepared.commit_into(self)
            }

            pub async fn commit_prepared_authority_after<T, E, F, Fut>(
                &mut self,
                prepared: #prepared_authority_name,
                effect: F,
            ) -> Result<T, #prepared_commit_effect_error_name<E>>
            where
                F: FnOnce() -> Fut,
                Fut: std::future::Future<Output = Result<T, E>>,
            {
                prepared
                    .validate_commit_base(self)
                    .map_err(#prepared_commit_effect_error_name::Commit)?;
                let result = effect()
                    .await
                    .map_err(#prepared_commit_effect_error_name::Effect)?;
                prepared.commit_into_unchecked(self);
                Ok(result)
            }

            pub fn generated_authority_owner_token(
                &self,
            ) -> std::sync::Arc<dyn std::any::Any + Send + Sync> {
                std::sync::Arc::clone(&self.authority_owner_token)
            }

            pub fn snapshot(&self) -> #authority_snapshot_name {
                #authority_snapshot_name {
                    state: self.state.clone(),
                }
            }

            pub fn restore_snapshot(&mut self, snapshot: #authority_snapshot_name) {
                self.state = snapshot.state;
            }

            #(#helpers)*

            #input_methods

            #signal_methods

            #signal_method
        }

        impl #mutator_trait for #authority_name {
            fn apply(&mut self, input: #input_name) -> Result<#transition_name, #error_name> {
                let from_phase = self.state.phase();
                let input_variant = input.variant();
                #[allow(unused_mut)]
                let mut effects = Vec::new();

                match input {
                    #input_arms
                    #[allow(unreachable_patterns)]
                    _ => return Err(#error_name::NoMatchingTransition {
                        phase: from_phase,
                        trigger: #trigger_name::Input(input_variant),
                    }),
                }

                let to_phase = self.state.phase();
                #(#invariant_checks)*
                Ok(#transition_name {
                    from_phase,
                    to_phase,
                    effects,
                    _origin: sealed::TransitionOrigin,
                })
            }
        }
    }
}

fn gen_input_match_arms_and_methods(
    def: &MachineDef,
    transitions: &[&TransitionDef],
    enum_name: &Ident,
    error_name: &Ident,
    variant_enum_name: &Ident,
    effect_name: &Ident,
    trigger_ctor: TokenStream,
) -> (TokenStream, TokenStream) {
    let mut groups: Vec<(&Ident, Vec<&&TransitionDef>)> = Vec::new();
    for t in transitions {
        let variant = &t.trigger.variant;
        if let Some(group) = groups.iter_mut().find(|(v, _)| *v == variant) {
            group.1.push(t);
        } else {
            groups.push((variant, vec![t]));
        }
    }

    let mut arms = Vec::new();
    let mut methods = Vec::new();
    for (variant, transitions) in groups {
        let method_name = format_ident!("apply_input_{}", variant.to_string().to_lowercase());
        let bindings: Vec<_> = transitions[0].trigger.bindings.iter().collect();
        let binding_pattern = if bindings.is_empty() {
            quote! {}
        } else {
            quote! { { #(#bindings),* } }
        };
        let call_args = if bindings.is_empty() {
            quote! {}
        } else {
            quote! { , #(#bindings),* }
        };
        arms.push(quote! {
            #enum_name::#variant #binding_pattern => {
                self.#method_name(from_phase, &mut effects #call_args)?;
            }
        });

        let Some(variant_def) = def
            .inputs
            .variants
            .iter()
            .find(|candidate| candidate.name == *variant)
        else {
            let message = format!("validated input variant '{variant}' should exist");
            return (quote! { compile_error!(#message); }, quote! {});
        };
        let mut params = Vec::new();
        for binding in &bindings {
            let Some(field) = variant_def
                .fields
                .iter()
                .find(|candidate| candidate.name == **binding)
            else {
                let message = format!(
                    "validated input binding '{binding}' should exist on variant '{variant}'"
                );
                return (quote! { compile_error!(#message); }, quote! {});
            };
            let ty = crate::gen_state::gen_type(&field.ty);
            params.push(quote! { #binding: #ty });
        }
        let method_params = if params.is_empty() {
            quote! {}
        } else {
            quote! { , #(#params),* }
        };
        let trigger = quote! { #trigger_ctor(#variant_enum_name::#variant) };
        let body = gen_transition_chain(def, &transitions, error_name, trigger);
        methods.push(quote! {
            #[allow(non_snake_case, clippy::ptr_arg, clippy::too_many_arguments)]
            fn #method_name(
                &mut self,
                from_phase: Phase,
                effects: &mut Vec<#effect_name>
                #method_params
            ) -> Result<(), #error_name> {
                #body
                Ok(())
            }
        });
    }

    (quote! { #(#arms)* }, quote! { #(#methods)* })
}

fn gen_signal_match_arms_and_methods(
    def: &MachineDef,
    transitions: &[&TransitionDef],
    enum_name: &Ident,
    error_name: &Ident,
    variant_enum_name: &Ident,
    effect_name: &Ident,
    trigger_ctor: TokenStream,
) -> (TokenStream, TokenStream) {
    let mut groups: Vec<(&Ident, Vec<&&TransitionDef>)> = Vec::new();
    for t in transitions {
        let variant = &t.trigger.variant;
        if let Some(group) = groups.iter_mut().find(|(v, _)| *v == variant) {
            group.1.push(t);
        } else {
            groups.push((variant, vec![t]));
        }
    }

    let mut arms = Vec::new();
    let mut methods = Vec::new();
    for (variant, transitions) in groups {
        let method_name = format_ident!("apply_signal_{}", variant.to_string().to_lowercase());
        let bindings: Vec<_> = transitions[0].trigger.bindings.iter().collect();
        let binding_pattern = if bindings.is_empty() {
            quote! {}
        } else {
            quote! { { #(#bindings),* } }
        };
        let call_args = if bindings.is_empty() {
            quote! {}
        } else {
            quote! { , #(#bindings),* }
        };
        arms.push(quote! {
            #enum_name::#variant #binding_pattern => {
                self.#method_name(from_phase, &mut effects #call_args)?;
            }
        });

        let Some(variant_def) = def
            .signals
            .variants
            .iter()
            .find(|candidate| candidate.name == *variant)
        else {
            let message = format!("validated signal variant '{variant}' should exist");
            return (quote! { compile_error!(#message); }, quote! {});
        };
        let mut params = Vec::new();
        for binding in &bindings {
            let Some(field) = variant_def
                .fields
                .iter()
                .find(|candidate| candidate.name == **binding)
            else {
                let message = format!(
                    "validated signal binding '{binding}' should exist on variant '{variant}'"
                );
                return (quote! { compile_error!(#message); }, quote! {});
            };
            let ty = crate::gen_state::gen_type(&field.ty);
            params.push(quote! { #binding: #ty });
        }
        let method_params = if params.is_empty() {
            quote! {}
        } else {
            quote! { , #(#params),* }
        };
        let trigger = quote! { #trigger_ctor(#variant_enum_name::#variant) };
        let body = gen_transition_chain(def, &transitions, error_name, trigger);
        methods.push(quote! {
            #[allow(non_snake_case, clippy::ptr_arg, clippy::too_many_arguments)]
            fn #method_name(
                &mut self,
                from_phase: Phase,
                effects: &mut Vec<#effect_name>
                #method_params
            ) -> Result<(), #error_name> {
                #body
                Ok(())
            }
        });
    }

    (quote! { #(#arms)* }, quote! { #(#methods)* })
}

fn gen_transition_chain(
    def: &MachineDef,
    transitions: &[&&TransitionDef],
    error_name: &Ident,
    trigger: TokenStream,
) -> TokenStream {
    if transitions.len() == 1 {
        let t = transitions[0];
        if t.has_guards() {
            let guard_exprs: Vec<_> = t
                .guards
                .iter()
                .map(|g| gen_expr(&g.expr, FieldPrefix::AuthorityState))
                .collect();
            let combined = quote! { #(#guard_exprs)&&* };
            let body = gen_transition_body(def, t);
            quote! {
                if #combined {
                    #body
                } else {
                    return Err(#error_name::GuardRejected {
                        phase: from_phase,
                        trigger: #trigger,
                    });
                }
            }
        } else {
            gen_transition_body(def, t)
        }
    } else {
        let mut arms = Vec::new();
        for (i, t) in transitions.iter().enumerate() {
            let body = gen_transition_body(def, t);
            if t.has_guards() {
                let guard_exprs: Vec<_> = t
                    .guards
                    .iter()
                    .map(|g| gen_expr(&g.expr, FieldPrefix::AuthorityState))
                    .collect();
                let combined = quote! { #(#guard_exprs)&&* };
                if i == 0 {
                    arms.push(quote! { if #combined { #body } });
                } else {
                    arms.push(quote! { else if #combined { #body } });
                }
            } else {
                arms.push(quote! { else { #body } });
            }
        }
        let last_has_guard = transitions.last().is_none_or(|t| t.has_guards());
        if last_has_guard {
            arms.push(quote! { else {
                return Err(#error_name::GuardRejected {
                    phase: from_phase,
                    trigger: #trigger,
                });
            } });
        }
        quote! { #(#arms)* }
    }
}

fn gen_transition_body(def: &MachineDef, t: &TransitionDef) -> TokenStream {
    let prefix = FieldPrefix::AuthorityState;
    let mut stmts = Vec::new();

    for update in &t.updates {
        stmts.push(gen_update(update, prefix));
    }

    if let Some(phase_field) = def.phase_field_name() {
        let phase_enum = &def.phase_enum.name;
        let to_phase = &t.to_phase;
        stmts.push(quote! { self.state.#phase_field = #phase_enum::#to_phase; });
    }

    let effect_name = &def.effects.name;
    for effect in &t.effects {
        let variant = &effect.variant;
        if effect.fields.is_empty() {
            stmts.push(quote! { effects.push(#effect_name::#variant); });
        } else {
            let field_assigns: Vec<_> = effect
                .fields
                .iter()
                .map(|(name, value)| {
                    let val = gen_expr(value, prefix);
                    // Clone to avoid move issues with String/Vec bindings
                    quote! { #name: (#val).clone() }
                })
                .collect();
            stmts.push(quote! { effects.push(#effect_name::#variant { #(#field_assigns),* }); });
        }
    }

    quote! { #(#stmts)* }
}

/// Generate Rust expression from an ExprDef.
pub(crate) fn gen_expr(expr: &ExprDef, prefix: FieldPrefix) -> TokenStream {
    match expr {
        ExprDef::Bool(v) => quote! { #v },
        ExprDef::U64(v) => quote! { #v },
        ExprDef::U64Max => quote! { u64::MAX },
        ExprDef::StringLit(s) => quote! { #s.to_string() },
        ExprDef::None => quote! { None },
        ExprDef::Some(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { Some(#e) }
        }
        ExprDef::EmptySeq => quote! { Vec::new() },
        ExprDef::EmptySet => quote! { std::collections::BTreeSet::new() },
        ExprDef::EmptyMap => quote! { std::collections::BTreeMap::new() },
        ExprDef::Field(name) => match prefix {
            FieldPrefix::AuthorityState => quote! { self.state.#name },
            FieldPrefix::DirectSelf => quote! { self.#name },
        },
        ExprDef::Binding(name) => quote! { #name.clone() },
        ExprDef::CurrentPhase => match prefix {
            FieldPrefix::AuthorityState => quote! { self.state.phase() },
            FieldPrefix::DirectSelf => quote! { self.phase() },
        },
        ExprDef::Phase(variant) => {
            quote! { Phase::#variant }
        }
        ExprDef::NamedVariant { enum_name, variant } => {
            quote! { #enum_name::#variant }
        }
        ExprDef::FieldAccess { base, field } => {
            let base = gen_expr(base, prefix);
            quote! { #base.#field.clone() }
        }
        ExprDef::EnumVariantIs {
            value,
            enum_name,
            variant,
            data_variant,
        } => {
            let value = gen_expr(value, prefix);
            if *data_variant {
                // Braced rest pattern matches both tuple and struct payload
                // variants, so `is_data_variant` is payload-shape agnostic.
                quote! { matches!(#value, #enum_name::#variant { .. }) }
            } else {
                quote! { matches!(#value, #enum_name::#variant) }
            }
        }
        ExprDef::EnumStringSetPayload {
            value,
            enum_name,
            variant,
            ..
        } => {
            let value = gen_expr(value, prefix);
            quote! {
                match #value {
                    #enum_name::#variant(payload) => payload.clone(),
                    _ => std::collections::BTreeSet::new(),
                }
            }
        }
        ExprDef::Not(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { !(#e) }
        }
        ExprDef::And(exprs) => {
            let parts: Vec<_> = exprs.iter().map(|e| gen_expr(e, prefix)).collect();
            quote! { (#(#parts)&&*) }
        }
        ExprDef::Or(exprs) => {
            let parts: Vec<_> = exprs.iter().map(|e| gen_expr(e, prefix)).collect();
            quote! { (#(#parts)||*) }
        }
        ExprDef::Eq(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left == #right) }
        }
        ExprDef::Neq(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left != #right) }
        }
        ExprDef::Gt(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left > #right) }
        }
        ExprDef::Gte(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left >= #right) }
        }
        ExprDef::Lt(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left < #right) }
        }
        ExprDef::Lte(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left <= #right) }
        }
        ExprDef::Add(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left + #right) }
        }
        ExprDef::Sub(l, r) => {
            let left = gen_expr(l, prefix);
            let right = gen_expr(r, prefix);
            quote! { (#left - #right) }
        }
        ExprDef::Contains { collection, value } => {
            let coll = gen_expr(collection, prefix);
            let val = gen_expr(value, prefix);
            quote! { #coll.contains(&#val) }
        }
        ExprDef::MapContainsKey { map, key } => {
            let m = gen_expr(map, prefix);
            let k = gen_expr(key, prefix);
            quote! { #m.contains_key(&#k) }
        }
        ExprDef::Len(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { #e.len() as u64 }
        }
        ExprDef::Count { collection, value } => {
            let collection = gen_expr(collection, prefix);
            let value = gen_expr(value, prefix);
            quote! { #collection.iter().filter(|candidate| *candidate == &#value).count() as u64 }
        }
        ExprDef::MapGet { map, key } => {
            let m = gen_expr(map, prefix);
            let k = gen_expr(key, prefix);
            quote! { #m.get(&#k) }
        }
        ExprDef::MapGetCopied { map, key } => {
            let m = gen_expr(map, prefix);
            let k = gen_expr(key, prefix);
            quote! { #m.get(&#k).copied() }
        }
        ExprDef::MapGetCloned { map, key } => {
            let m = gen_expr(map, prefix);
            let k = gen_expr(key, prefix);
            quote! { #m.get(&#k).cloned() }
        }
        ExprDef::MapKeys(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { #e.keys().cloned().collect::<std::collections::BTreeSet<_>>() }
        }
        ExprDef::IsSome(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { #e.is_some() }
        }
        ExprDef::IsNone(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { #e.is_none() }
        }
        ExprDef::ForAll {
            binding,
            over,
            body,
        } => {
            let over_e = gen_expr(over, prefix);
            let body_e = gen_expr(body, prefix);
            quote! { #over_e.iter().all(|#binding| #body_e) }
        }
        ExprDef::Exists {
            binding,
            over,
            body,
        } => {
            let over_e = gen_expr(over, prefix);
            let body_e = gen_expr(body, prefix);
            quote! { #over_e.iter().any(|#binding| #body_e) }
        }
        ExprDef::Call { helper, args } => {
            let arg_exprs: Vec<_> = args
                .iter()
                .map(|a| {
                    let e = gen_expr(a, prefix);
                    quote! { &#e }
                })
                .collect();
            quote! { Self::#helper(#(#arg_exprs),*) }
        }
        ExprDef::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            let cond = gen_expr(condition, prefix);
            let then_e = gen_expr(then_expr, prefix);
            let else_e = gen_expr(else_expr, prefix);
            quote! { if #cond { #then_e } else { #else_e } }
        }
    }
}

/// Convenience alias for authority-context expressions.
#[allow(dead_code)]
pub(crate) fn gen_guard_expr(expr: &ExprDef) -> TokenStream {
    gen_expr(expr, FieldPrefix::AuthorityState)
}

fn gen_update(update: &UpdateDef, prefix: FieldPrefix) -> TokenStream {
    match update {
        UpdateDef::Assign { field, value } => {
            let val = gen_expr(value, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field = #val; },
                FieldPrefix::DirectSelf => quote! { self.#field = #val; },
            }
        }
        UpdateDef::Increment { field, amount } => {
            let amt = gen_expr(amount, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field += #amt; },
                FieldPrefix::DirectSelf => quote! { self.#field += #amt; },
            }
        }
        UpdateDef::Decrement { field, amount } => {
            let amt = gen_expr(amount, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field -= #amt; },
                FieldPrefix::DirectSelf => quote! { self.#field -= #amt; },
            }
        }
        UpdateDef::SetInsert { field, value } => {
            let val = gen_expr(value, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field.insert(#val); },
                FieldPrefix::DirectSelf => quote! { self.#field.insert(#val); },
            }
        }
        UpdateDef::SetRemove { field, value } => {
            let val = gen_expr(value, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field.remove(&#val); },
                FieldPrefix::DirectSelf => quote! { self.#field.remove(&#val); },
            }
        }
        UpdateDef::MapInsert { field, key, value } => {
            let k = gen_expr(key, prefix);
            let v = gen_expr(value, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field.insert(#k, #v); },
                FieldPrefix::DirectSelf => quote! { self.#field.insert(#k, #v); },
            }
        }
        UpdateDef::MapRemove { field, key } => {
            let k = gen_expr(key, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field.remove(&#k); },
                FieldPrefix::DirectSelf => quote! { self.#field.remove(&#k); },
            }
        }
        UpdateDef::MapIncrement { field, key, amount } => {
            let k = gen_expr(key, prefix);
            let amt = gen_expr(amount, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! {
                    {
                        let entry = self.state.#field.entry(#k).or_insert(0);
                        *entry = entry.saturating_add(#amt);
                    }
                },
                FieldPrefix::DirectSelf => quote! {
                    {
                        let entry = self.#field.entry(#k).or_insert(0);
                        *entry = entry.saturating_add(#amt);
                    }
                },
            }
        }
        UpdateDef::MapDecrement { field, key, amount } => {
            let k = gen_expr(key, prefix);
            let amt = gen_expr(amount, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! {
                    {
                        let entry = self.state.#field.entry(#k).or_insert(0);
                        *entry = entry.saturating_sub(#amt);
                    }
                },
                FieldPrefix::DirectSelf => quote! {
                    {
                        let entry = self.#field.entry(#k).or_insert(0);
                        *entry = entry.saturating_sub(#amt);
                    }
                },
            }
        }
        UpdateDef::Conditional {
            condition,
            then_updates,
            else_updates,
        } => {
            let cond = gen_expr(condition, prefix);
            let then_stmts: Vec<_> = then_updates.iter().map(|u| gen_update(u, prefix)).collect();
            let else_stmts: Vec<_> = else_updates.iter().map(|u| gen_update(u, prefix)).collect();
            if else_stmts.is_empty() {
                quote! { if #cond { #(#then_stmts)* } }
            } else {
                quote! { if #cond { #(#then_stmts)* } else { #(#else_stmts)* } }
            }
        }
    }
}

fn gen_helper(helper: &HelperDef) -> TokenStream {
    let name = &helper.name;
    let return_ty = crate::gen_state::gen_type(&helper.return_ty);
    let body = gen_expr(&helper.body, FieldPrefix::AuthorityState);

    let params: Vec<_> = helper
        .params
        .iter()
        .map(|p| {
            let pname = &p.name;
            let pty = crate::gen_state::gen_type(&p.ty);
            quote! { #pname: &#pty }
        })
        .collect();

    if helper.params.is_empty() {
        quote! {
            fn #name(&self) -> #return_ty {
                #body
            }
        }
    } else {
        quote! {
            fn #name(#(#params),*) -> #return_ty {
                #body
            }
        }
    }
}
