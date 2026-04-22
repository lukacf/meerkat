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
    let phase_name = &def.phase_enum.name;
    let effect_name = &def.effects.name;
    let machine_name = &def.name;

    let transition_name = format_ident!("{}Transition", machine_name);
    let error_name = format_ident!("{}TransitionError", machine_name);
    let authority_name = format_ident!("{}Authority", machine_name);
    let mutator_trait = format_ident!("{}Mutator", machine_name);
    let transition_id_name = format_ident!("{}TransitionId", machine_name);
    let guard_id_name = format_ident!("{}GuardId", machine_name);
    let helper_id_name = format_ident!("{}HelperId", machine_name);

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

    let input_arms = gen_match_arms(def, &input_transitions, input_name, &error_name);

    let has_signals = !def.signals.variants.is_empty() && !signal_transitions.is_empty();
    let signal_name = &def.signals.name;
    let signal_arms = if has_signals {
        gen_match_arms(def, &signal_transitions, signal_name, &error_name)
    } else {
        TokenStream::new()
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

    let helpers: Vec<_> = def.helpers.iter().map(gen_helper).collect();
    let transition_id_variants: Vec<_> = def.transitions.iter().map(|t| &t.name).collect();
    let helper_id_variants: Vec<_> = def
        .helpers
        .iter()
        .map(|helper| pascal_case_ident(&helper.name.to_string()))
        .collect();
    let guard_id_variants: Vec<_> = def
        .transitions
        .iter()
        .flat_map(|transition| {
            transition
                .guards
                .iter()
                .enumerate()
                .map(move |(idx, _)| format_ident!("{}Guard{}", transition.name, idx + 1))
        })
        .collect();

    let signal_method = if has_signals {
        quote! {
            pub fn apply_signal(&mut self, signal: #signal_name) -> Result<#transition_name, #error_name> {
                let from_phase = self.state.phase();
                let mut effects = Vec::new();
                let transition_id;

                match signal {
                    #signal_arms
                    #[allow(unreachable_patterns)]
                    _ => return Err(#error_name::NoMatchingTransition {
                        phase: format!("{:?}", from_phase),
                        trigger: format!("{:?}", signal),
                    }),
                }

                let to_phase = self.state.phase();
                #(#invariant_checks)*
                Ok(#transition_name { from_phase, to_phase, transition_id, effects })
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
            pub transition_id: #transition_id_name,
            pub effects: Vec<#effect_name>,
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum #transition_id_name {
            #(#transition_id_variants),*
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum #guard_id_name {
            #(#guard_id_variants),*
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum #helper_id_name {
            #(#helper_id_variants),*
        }

        #[derive(Debug, Clone, PartialEq, Eq)]
        pub enum #error_name {
            /// No transition is declared for this `(phase, trigger)` pair at
            /// all — the trigger variant is semantically out of scope for
            /// the current phase. Shell callers should treat this as a hard
            /// error: firing the wrong input for the current phase is a
            /// programming mistake.
            NoMatchingTransition { phase: String, trigger: String },
            /// A transition is declared for this `(phase, trigger)` pair
            /// but every candidate transition's guard(s) evaluated false.
            /// Typed signal that the input was *rejected on state*, not
            /// *unrecognised*. Shell callers that fire idempotently (e.g.,
            /// a realtime dispatcher firing `ProductTurnCommitted` on every
            /// observed `TurnCommitted` event, expecting the DSL to drop
            /// duplicates) should treat this as a successful no-op rather
            /// than an error.
            GuardRejected { phase: String, trigger: String },
        }

        impl std::fmt::Display for #error_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::NoMatchingTransition { phase, trigger } => {
                        write!(f, "no matching transition from phase {} for {}", phase, trigger)
                    }
                    Self::GuardRejected { phase, trigger } => {
                        write!(f, "guard rejected transition from phase {} for {}", phase, trigger)
                    }
                }
            }
        }

        impl std::error::Error for #error_name {}

        mod sealed {
            pub trait Sealed {}
        }

        pub trait #mutator_trait: sealed::Sealed {
            fn apply(&mut self, input: #input_name) -> Result<#transition_name, #error_name>;
        }

        pub struct #authority_name {
            pub state: #state_name,
        }

        impl sealed::Sealed for #authority_name {}

        impl #authority_name {
            pub fn new() -> Self {
                Self { state: #state_name::default() }
            }

            pub fn from_state(state: #state_name) -> Self {
                Self { state }
            }

            #(#helpers)*

            #signal_method
        }

        impl #mutator_trait for #authority_name {
            fn apply(&mut self, input: #input_name) -> Result<#transition_name, #error_name> {
                let from_phase = self.state.phase();
                #[allow(unused_mut)]
                let mut effects = Vec::new();
                let transition_id;

                match input {
                    #input_arms
                    #[allow(unreachable_patterns)]
                    _ => return Err(#error_name::NoMatchingTransition {
                        phase: format!("{:?}", from_phase),
                        trigger: format!("{:?}", input),
                    }),
                }

                let to_phase = self.state.phase();
                #(#invariant_checks)*
                Ok(#transition_name { from_phase, to_phase, transition_id, effects })
            }
        }
    }
}

fn gen_match_arms(
    def: &MachineDef,
    transitions: &[&TransitionDef],
    enum_name: &Ident,
    error_name: &Ident,
) -> TokenStream {
    let mut groups: Vec<(&Ident, Vec<&&TransitionDef>)> = Vec::new();
    for t in transitions {
        let variant = &t.trigger.variant;
        if let Some(group) = groups.iter_mut().find(|(v, _)| *v == variant) {
            group.1.push(t);
        } else {
            groups.push((variant, vec![t]));
        }
    }

    let arms: Vec<_> = groups
        .iter()
        .map(|(variant, transitions)| {
            let binding_pattern = if transitions[0].trigger.bindings.is_empty() {
                quote! {}
            } else {
                let bindings: Vec<_> = transitions[0].trigger.bindings.iter().collect();
                quote! { { #(#bindings),* } }
            };

            let body = gen_transition_chain(def, transitions, error_name);

            quote! {
                #enum_name::#variant #binding_pattern => {
                    #body
                }
            }
        })
        .collect();

    quote! { #(#arms)* }
}

fn gen_transition_chain(
    def: &MachineDef,
    transitions: &[&&TransitionDef],
    error_name: &Ident,
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
            let variant_str = t.trigger.variant.to_string();
            quote! {
                if #combined {
                    #body
                } else {
                    return Err(#error_name::GuardRejected {
                        phase: format!("{:?}", from_phase),
                        trigger: #variant_str.to_string(),
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
            let variant_str = transitions[0].trigger.variant.to_string();
            arms.push(quote! { else {
                return Err(#error_name::GuardRejected {
                    phase: format!("{:?}", from_phase),
                    trigger: #variant_str.to_string(),
                });
            } });
        }
        quote! { #(#arms)* }
    }
}

fn gen_transition_body(def: &MachineDef, t: &TransitionDef) -> TokenStream {
    let prefix = FieldPrefix::AuthorityState;
    let mut stmts = Vec::new();
    let transition_id_name = format_ident!("{}TransitionId", def.name);
    let transition_id_variant = &t.name;

    stmts.push(quote! {
        transition_id = #transition_id_name::#transition_id_variant;
    });

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

fn pascal_case_ident(raw: &str) -> Ident {
    let mut out = String::new();
    let mut capitalize = true;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            if capitalize {
                out.push(ch.to_ascii_uppercase());
                capitalize = false;
            } else {
                out.push(ch);
            }
        } else {
            capitalize = true;
        }
    }
    if out.is_empty() {
        out.push('X');
    }
    format_ident!("{out}")
}

/// Generate Rust expression from an ExprDef.
pub(crate) fn gen_expr(expr: &ExprDef, prefix: FieldPrefix) -> TokenStream {
    match expr {
        ExprDef::Bool(v) => quote! { #v },
        ExprDef::U64(v) => {
            let lit = proc_macro2::Literal::u64_unsuffixed(*v);
            quote! { #lit }
        }
        ExprDef::StringLit(s) => quote! { #s.to_string() },
        ExprDef::None => quote! { None },
        ExprDef::Some(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { Some(#e) }
        }
        ExprDef::EmptySet => quote! { std::collections::BTreeSet::new() },
        ExprDef::EmptyMap => quote! { std::collections::BTreeMap::new() },
        ExprDef::SeqLiteral(items) => {
            let values: Vec<_> = items.iter().map(|item| gen_expr(item, prefix)).collect();
            quote! { vec![#(#values),*] }
        }
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
        ExprDef::ConstructNamed { type_name, value } => {
            quote! { #type_name(#value.to_string()) }
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
        ExprDef::Eq(l, r) => gen_comparison_expr(l, r, prefix, false),
        ExprDef::Neq(l, r) => gen_comparison_expr(l, r, prefix, true),
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
        ExprDef::SeqStartsWith {
            seq,
            prefix: seq_prefix,
        } => {
            let seq_e = gen_expr(seq, prefix);
            let prefix_e = gen_expr(seq_prefix, prefix);
            quote! { #seq_e.starts_with(&#prefix_e) }
        }
        ExprDef::SeqElements(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { #e.iter().cloned().collect::<std::collections::BTreeSet<_>>() }
        }
        ExprDef::Len(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { #e.len() }
        }
        ExprDef::Head(inner) => {
            let e = gen_expr(inner, prefix);
            quote! { #e.first().cloned().expect("Head on empty sequence") }
        }
        ExprDef::MapGet { map, key } => {
            let m = gen_expr(map, prefix);
            let k = gen_expr(key, prefix);
            quote! { #m.get(&#k) }
        }
        ExprDef::MapGetFlatten { map, key } => {
            let m = gen_expr(map, prefix);
            let k = gen_expr(key, prefix);
            quote! { #m.get(&#k).and_then(|value| value.as_ref()) }
        }
        ExprDef::MapGetCloned { map, key } => {
            let m = gen_expr(map, prefix);
            let k = gen_expr(key, prefix);
            quote! { #m.get(&#k).cloned().expect("missing map entry") }
        }
        ExprDef::MapGetCopied { map, key } => {
            let m = gen_expr(map, prefix);
            let k = gen_expr(key, prefix);
            quote! { #m.get(&#k).copied() }
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
            quote! { self.#helper(#(#arg_exprs),*) }
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

fn gen_comparison_expr(
    left: &ExprDef,
    right: &ExprDef,
    prefix: FieldPrefix,
    invert: bool,
) -> TokenStream {
    match (left, right) {
        (ExprDef::MapGet { .. }, ExprDef::None) => {
            let left_e = gen_expr(left, prefix);
            if invert {
                quote! { (#left_e.is_some()) }
            } else {
                quote! { (#left_e.is_none()) }
            }
        }
        (ExprDef::MapGetFlatten { .. }, ExprDef::None) => {
            let left_e = gen_expr(left, prefix);
            if invert {
                quote! { (#left_e.is_some()) }
            } else {
                quote! { (#left_e.is_none()) }
            }
        }
        (ExprDef::MapGet { .. }, ExprDef::Some(inner)) => {
            let left_e = gen_expr(left, prefix);
            let inner_e = gen_expr(inner, prefix);
            if invert {
                quote! { (#left_e != Some(&(Some(#inner_e)))) }
            } else {
                quote! { (#left_e == Some(&(Some(#inner_e)))) }
            }
        }
        (ExprDef::MapGetFlatten { .. }, ExprDef::Some(inner)) => {
            let left_e = gen_expr(left, prefix);
            let inner_e = gen_expr(inner, prefix);
            if invert {
                quote! { (#left_e != Some(&(#inner_e))) }
            } else {
                quote! { (#left_e == Some(&(#inner_e))) }
            }
        }
        (ExprDef::MapGet { .. }, _) => {
            let left_e = gen_expr(left, prefix);
            let right_e = gen_expr(right, prefix);
            if invert {
                quote! { (#left_e != Some(&(#right_e))) }
            } else {
                quote! { (#left_e == Some(&(#right_e))) }
            }
        }
        (ExprDef::MapGetFlatten { .. }, _) => {
            let left_e = gen_expr(left, prefix);
            let right_e = gen_expr(right, prefix);
            if invert {
                quote! { (#left_e != Some(&(#right_e))) }
            } else {
                quote! { (#left_e == Some(&(#right_e))) }
            }
        }
        (ExprDef::None, ExprDef::MapGet { .. }) => {
            let right_e = gen_expr(right, prefix);
            if invert {
                quote! { (#right_e.is_some()) }
            } else {
                quote! { (#right_e.is_none()) }
            }
        }
        (ExprDef::None, ExprDef::MapGetFlatten { .. }) => {
            let right_e = gen_expr(right, prefix);
            if invert {
                quote! { (#right_e.is_some()) }
            } else {
                quote! { (#right_e.is_none()) }
            }
        }
        (ExprDef::Some(inner), ExprDef::MapGet { .. }) => {
            let inner_e = gen_expr(inner, prefix);
            let right_e = gen_expr(right, prefix);
            if invert {
                quote! { (Some(&(Some(#inner_e))) != #right_e) }
            } else {
                quote! { (Some(&(Some(#inner_e))) == #right_e) }
            }
        }
        (ExprDef::Some(inner), ExprDef::MapGetFlatten { .. }) => {
            let inner_e = gen_expr(inner, prefix);
            let right_e = gen_expr(right, prefix);
            if invert {
                quote! { (Some(&(#inner_e)) != #right_e) }
            } else {
                quote! { (Some(&(#inner_e)) == #right_e) }
            }
        }
        (_, ExprDef::MapGet { .. }) => {
            let left_e = gen_expr(left, prefix);
            let right_e = gen_expr(right, prefix);
            if invert {
                quote! { (Some(&(#left_e)) != #right_e) }
            } else {
                quote! { (Some(&(#left_e)) == #right_e) }
            }
        }
        (_, ExprDef::MapGetFlatten { .. }) => {
            let left_e = gen_expr(left, prefix);
            let right_e = gen_expr(right, prefix);
            if invert {
                quote! { (Some(&(#left_e)) != #right_e) }
            } else {
                quote! { (Some(&(#left_e)) == #right_e) }
            }
        }
        _ => {
            let left_e = gen_expr(left, prefix);
            let right_e = gen_expr(right, prefix);
            if invert {
                quote! { (#left_e != #right_e) }
            } else {
                quote! { (#left_e == #right_e) }
            }
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
        UpdateDef::SeqAppend { field, value } => {
            let val = gen_expr(value, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! { self.state.#field.push(#val); },
                FieldPrefix::DirectSelf => quote! { self.#field.push(#val); },
            }
        }
        UpdateDef::SeqPrepend { field, values } => {
            let vals = gen_expr(values, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! {
                    {
                        let mut prefix_values = #vals;
                        prefix_values.extend(self.state.#field.clone());
                        self.state.#field = prefix_values;
                    }
                },
                FieldPrefix::DirectSelf => quote! {
                    {
                        let mut prefix_values = #vals;
                        prefix_values.extend(self.#field.clone());
                        self.#field = prefix_values;
                    }
                },
            }
        }
        UpdateDef::SeqPopFront { field } => match prefix {
            FieldPrefix::AuthorityState => quote! {
                if !self.state.#field.is_empty() {
                    self.state.#field.remove(0);
                }
            },
            FieldPrefix::DirectSelf => quote! {
                if !self.#field.is_empty() {
                    self.#field.remove(0);
                }
            },
        },
        UpdateDef::SeqRemoveValue { field, value } => {
            let val = gen_expr(value, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! {
                    self.state.#field.retain(|item| item != &#val);
                },
                FieldPrefix::DirectSelf => quote! {
                    self.#field.retain(|item| item != &#val);
                },
            }
        }
        UpdateDef::SeqRemoveAll { field, values } => {
            let vals = gen_expr(values, prefix);
            match prefix {
                FieldPrefix::AuthorityState => quote! {
                    {
                        let values = #vals;
                        self.state.#field.retain(|item| !values.contains(item));
                    }
                },
                FieldPrefix::DirectSelf => quote! {
                    {
                        let values = #vals;
                        self.#field.retain(|item| !values.contains(item));
                    }
                },
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
        UpdateDef::ForEach {
            binding,
            over,
            updates,
        } => {
            let over_e = gen_expr(over, prefix);
            let update_stmts: Vec<_> = updates.iter().map(|u| gen_update(u, prefix)).collect();
            quote! {
                for #binding in #over_e.iter() {
                    #(#update_stmts)*
                }
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

    quote! {
        pub fn #name(&self #(, #params)*) -> #return_ty {
            #body
        }
    }
}
