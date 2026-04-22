use proc_macro2::TokenStream;
use quote::quote;

use crate::ast::{ExprDef, MachineDef, TransitionDef, UpdateDef};

/// Belt-and-suspenders: emit a `compile_error!` token if a non-literal
/// arithmetic amount ever reaches codegen. `validate::validate_arithmetic_amount`
/// already rejects these, but this keeps the codegen total — it never silently
/// coerces to `1`.
fn non_literal_amount_compile_error(field: &str, update_kind: &str) -> TokenStream {
    let msg = format!(
        "internal error: {update_kind} amount for field `{field}` was not a u64 literal \
         at schema codegen (validation should have caught this)"
    );
    quote! { compile_error!(#msg) }
}

/// Generate `fn schema() -> meerkat_machine_schema::MachineSchema`.
///
/// This is the backward-compatibility bridge: codegen, RMAT, validation, and
/// xtask all work with MachineSchema. The invoking crate must depend on
/// `meerkat-machine-schema` — we just emit the constructor tokens.
pub fn generate(def: &MachineDef) -> TokenStream {
    let machine_name = def.name.to_string();
    let version = def.version;
    let rust_crate = &def.rust_crate;
    let rust_module = &def.rust_module;

    let phase_enum_name = def.phase_enum.name.to_string();
    let phase_variants = gen_variants(&def.phase_enum.variants);

    let state_fields = gen_state_fields(def);
    let init_phase = def.init_phase.to_string();
    let init_fields = gen_init_fields(def);
    let terminal_phases: Vec<_> = def
        .terminal_phases
        .iter()
        .map(std::string::ToString::to_string)
        .collect();

    let input_name = def.inputs.name.to_string();
    let input_variants = gen_enum_variants(&def.inputs);

    let surface_only: Vec<_> = def
        .surface_only_inputs
        .iter()
        .map(std::string::ToString::to_string)
        .collect();

    let signal_name = def.signals.name.to_string();
    let signal_variants = gen_enum_variants(&def.signals);

    let effect_name = def.effects.name.to_string();
    let effect_variants = gen_enum_variants(&def.effects);

    let helpers = gen_helpers(def);
    let invariants = gen_invariants(def);
    let transitions = gen_transitions(def);
    let dispositions = gen_dispositions(def);

    let state_name = crate::gen_state::state_struct_name(def);

    // When rust_crate is "self", the DSL is invoked inside meerkat-machine-schema
    // itself, so imports use `crate::` instead of `meerkat_machine_schema::`.
    let schema_crate = if def.rust_crate == "self" {
        quote! { crate }
    } else {
        quote! { meerkat_machine_schema }
    };

    quote! {
        impl #state_name {
            pub fn schema() -> #schema_crate::MachineSchema {
                use #schema_crate::*;

                MachineSchema {
                    machine: #machine_name.into(),
                    version: #version,
                    rust: RustBinding {
                        crate_name: #rust_crate.into(),
                        module: #rust_module.into(),
                    },
                    state: StateSchema {
                        phase: EnumSchema {
                            name: #phase_enum_name.into(),
                            variants: vec![#(#phase_variants),*],
                        },
                        fields: vec![#(#state_fields),*],
                        init: InitSchema {
                            phase: #init_phase.into(),
                            fields: vec![#(#init_fields),*],
                        },
                        terminal_phases: vec![#(#terminal_phases.into()),*],
                    },
                    inputs: EnumSchema {
                        name: #input_name.into(),
                        variants: vec![#(#input_variants),*],
                    },
                    surface_only_inputs: vec![#(#surface_only.into()),*],
                    signals: EnumSchema {
                        name: #signal_name.into(),
                        variants: vec![#(#signal_variants),*],
                    },
                    effects: EnumSchema {
                        name: #effect_name.into(),
                        variants: vec![#(#effect_variants),*],
                    },
                    helpers: vec![#(#helpers),*],
                    derived: vec![],
                    invariants: vec![#(#invariants),*],
                    transitions: vec![#(#transitions),*],
                    effect_dispositions: vec![#(#dispositions),*],
                    ci_step_limit: None,
                }
            }
        }
    }
}

fn gen_variants(idents: &[syn::Ident]) -> Vec<TokenStream> {
    idents
        .iter()
        .map(|v| {
            let name = v.to_string();
            quote! {
                VariantSchema { name: #name.into(), fields: vec![] }
            }
        })
        .collect()
}

fn gen_type_ref(ty: &crate::ast::TypeDef) -> TokenStream {
    match ty {
        crate::ast::TypeDef::Bool => quote! { TypeRef::Bool },
        crate::ast::TypeDef::U32 => quote! { TypeRef::U32 },
        crate::ast::TypeDef::U64 => quote! { TypeRef::U64 },
        crate::ast::TypeDef::String => quote! { TypeRef::String },
        crate::ast::TypeDef::Option(inner) => {
            let inner_ref = gen_type_ref(inner);
            quote! { TypeRef::Option(Box::new(#inner_ref)) }
        }
        crate::ast::TypeDef::Seq(inner) => {
            let inner_ref = gen_type_ref(inner);
            quote! { TypeRef::Seq(Box::new(#inner_ref)) }
        }
        crate::ast::TypeDef::Set(inner) => {
            let inner_ref = gen_type_ref(inner);
            quote! { TypeRef::Set(Box::new(#inner_ref)) }
        }
        crate::ast::TypeDef::Map(k, v) => {
            let key_ref = gen_type_ref(k);
            let val_ref = gen_type_ref(v);
            quote! { TypeRef::Map(Box::new(#key_ref), Box::new(#val_ref)) }
        }
        crate::ast::TypeDef::Named(ident) => {
            let name = ident.to_string();
            quote! { TypeRef::Named(#name.into()) }
        }
        crate::ast::TypeDef::Enum(ident) => {
            let name = ident.to_string();
            quote! { TypeRef::Enum(#name.into()) }
        }
    }
}

fn gen_state_fields(def: &MachineDef) -> Vec<TokenStream> {
    let phase_field_name = def.phase_field_name().map(std::string::ToString::to_string);
    def.state_fields
        .iter()
        // Exclude the stored-phase field — the schema models phase separately
        .filter(|f| Some(f.name.to_string()) != phase_field_name)
        .map(|f| {
            let name = f.name.to_string();
            let ty = gen_type_ref(&f.ty);
            quote! {
                FieldSchema { name: #name.into(), ty: #ty }
            }
        })
        .collect()
}

fn gen_init_fields(def: &MachineDef) -> Vec<TokenStream> {
    let mut fields = Vec::new();

    // Note: the phase init is handled by InitSchema.phase, not as a field init.
    // So we skip it here even for stored-phase machines.

    for init in &def.init_fields {
        let name = init.name.to_string();
        let expr = gen_schema_expr(&init.value);
        fields.push(quote! {
            FieldInit { field: #name.into(), expr: #expr }
        });
    }
    fields
}

/// Rewrite Field(phase_field) → CurrentPhase in an expression tree.
/// Used for stored-phase machines where the phase field isn't in the schema's
/// field list — the schema uses CurrentPhase instead.
fn rewrite_phase_field_to_current(expr: &ExprDef, phase_field: &str) -> ExprDef {
    match expr {
        ExprDef::Field(name) if name == phase_field => ExprDef::CurrentPhase,
        ExprDef::Not(inner) => {
            ExprDef::Not(Box::new(rewrite_phase_field_to_current(inner, phase_field)))
        }
        ExprDef::And(exprs) => ExprDef::And(
            exprs
                .iter()
                .map(|e| rewrite_phase_field_to_current(e, phase_field))
                .collect(),
        ),
        ExprDef::Or(exprs) => ExprDef::Or(
            exprs
                .iter()
                .map(|e| rewrite_phase_field_to_current(e, phase_field))
                .collect(),
        ),
        ExprDef::Eq(l, r) => ExprDef::Eq(
            Box::new(rewrite_phase_field_to_current(l, phase_field)),
            Box::new(rewrite_phase_field_to_current(r, phase_field)),
        ),
        ExprDef::Neq(l, r) => ExprDef::Neq(
            Box::new(rewrite_phase_field_to_current(l, phase_field)),
            Box::new(rewrite_phase_field_to_current(r, phase_field)),
        ),
        ExprDef::IsSome(inner) => {
            ExprDef::IsSome(Box::new(rewrite_phase_field_to_current(inner, phase_field)))
        }
        ExprDef::IsNone(inner) => {
            ExprDef::IsNone(Box::new(rewrite_phase_field_to_current(inner, phase_field)))
        }
        ExprDef::Call { helper, args } => ExprDef::Call {
            helper: helper.clone(),
            args: args
                .iter()
                .map(|a| rewrite_phase_field_to_current(a, phase_field))
                .collect(),
        },
        _ => expr.clone(),
    }
}

/// Generate schema expression, rewriting phase field references if needed.
fn gen_schema_expr_for(def: &MachineDef, expr: &ExprDef) -> TokenStream {
    if let Some(pf) = def.phase_field_name() {
        let rewritten = rewrite_phase_field_to_current(expr, &pf.to_string());
        gen_schema_expr(&rewritten)
    } else {
        gen_schema_expr(expr)
    }
}

fn gen_schema_expr(expr: &ExprDef) -> TokenStream {
    match expr {
        ExprDef::Bool(v) => quote! { Expr::Bool(#v) },
        ExprDef::U64(v) => quote! { Expr::U64(#v) },
        ExprDef::StringLit(s) => quote! { Expr::String(#s.into()) },
        ExprDef::None => quote! { Expr::None },
        ExprDef::Some(inner) => {
            let inner_e = gen_schema_expr(inner);
            quote! { Expr::Some(Box::new(#inner_e)) }
        }
        ExprDef::EmptySeq => quote! { Expr::SeqLiteral(vec![]) },
        ExprDef::EmptySet => quote! { Expr::EmptySet },
        ExprDef::EmptyMap => quote! { Expr::EmptyMap },
        ExprDef::Field(name) => {
            let n = name.to_string();
            quote! { Expr::Field(#n.into()) }
        }
        ExprDef::Binding(name) => {
            let n = name.to_string();
            quote! { Expr::Binding(#n.into()) }
        }
        ExprDef::CurrentPhase => quote! { Expr::CurrentPhase },
        ExprDef::Phase(variant) => {
            let v = variant.to_string();
            quote! { Expr::Phase(#v.into()) }
        }
        ExprDef::NamedVariant { enum_name, variant } => {
            let e = enum_name.to_string();
            let v = variant.to_string();
            quote! { Expr::NamedVariant { enum_name: #e.into(), variant: #v.into() } }
        }
        ExprDef::Not(inner) => {
            let inner_e = gen_schema_expr(inner);
            quote! { Expr::Not(Box::new(#inner_e)) }
        }
        ExprDef::And(exprs) => {
            let parts: Vec<_> = exprs.iter().map(gen_schema_expr).collect();
            quote! { Expr::And(vec![#(#parts),*]) }
        }
        ExprDef::Or(exprs) => {
            let parts: Vec<_> = exprs.iter().map(gen_schema_expr).collect();
            quote! { Expr::Or(vec![#(#parts),*]) }
        }
        ExprDef::Eq(l, r) => {
            let left = gen_schema_expr(l);
            let right = gen_schema_expr(r);
            quote! { Expr::Eq(Box::new(#left), Box::new(#right)) }
        }
        ExprDef::Neq(l, r) => {
            let left = gen_schema_expr(l);
            let right = gen_schema_expr(r);
            quote! { Expr::Neq(Box::new(#left), Box::new(#right)) }
        }
        ExprDef::Gt(l, r) => {
            let left = gen_schema_expr(l);
            let right = gen_schema_expr(r);
            quote! { Expr::Gt(Box::new(#left), Box::new(#right)) }
        }
        ExprDef::Gte(l, r) => {
            let left = gen_schema_expr(l);
            let right = gen_schema_expr(r);
            quote! { Expr::Gte(Box::new(#left), Box::new(#right)) }
        }
        ExprDef::Lt(l, r) => {
            let left = gen_schema_expr(l);
            let right = gen_schema_expr(r);
            quote! { Expr::Lt(Box::new(#left), Box::new(#right)) }
        }
        ExprDef::Lte(l, r) => {
            let left = gen_schema_expr(l);
            let right = gen_schema_expr(r);
            quote! { Expr::Lte(Box::new(#left), Box::new(#right)) }
        }
        ExprDef::Add(l, r) => {
            let left = gen_schema_expr(l);
            let right = gen_schema_expr(r);
            quote! { Expr::Add(Box::new(#left), Box::new(#right)) }
        }
        ExprDef::Sub(l, r) => {
            let left = gen_schema_expr(l);
            let right = gen_schema_expr(r);
            quote! { Expr::Sub(Box::new(#left), Box::new(#right)) }
        }
        ExprDef::Contains { collection, value } => {
            let coll = gen_schema_expr(collection);
            let val = gen_schema_expr(value);
            quote! { Expr::Contains { collection: Box::new(#coll), value: Box::new(#val) } }
        }
        ExprDef::MapContainsKey { map, key } => {
            let m = gen_schema_expr(map);
            let k = gen_schema_expr(key);
            quote! { Expr::MapContainsKey { map: Box::new(#m), key: Box::new(#k) } }
        }
        ExprDef::Len(inner) => {
            let inner_e = gen_schema_expr(inner);
            quote! { Expr::Len(Box::new(#inner_e)) }
        }
        ExprDef::MapGet { map, key } => {
            let map_e = gen_schema_expr(map);
            let key_e = gen_schema_expr(key);
            quote! { Expr::MapGet { map: Box::new(#map_e), key: Box::new(#key_e) } }
        }
        ExprDef::MapGetCopied { map, key } => {
            // Schema-side, `get_copied` is indistinguishable from `get` — both
            // are lookups into the same map keyed on the same expression. The
            // `.copied()` difference is a Rust codegen detail.
            let map_e = gen_schema_expr(map);
            let key_e = gen_schema_expr(key);
            quote! { Expr::MapGet { map: Box::new(#map_e), key: Box::new(#key_e) } }
        }
        ExprDef::MapKeys(inner) => {
            let inner_e = gen_schema_expr(inner);
            quote! { Expr::MapKeys(Box::new(#inner_e)) }
        }
        ExprDef::IsSome(inner) => {
            let inner_e = gen_schema_expr(inner);
            quote! { Expr::Neq(Box::new(#inner_e), Box::new(Expr::None)) }
        }
        ExprDef::IsNone(inner) => {
            let inner_e = gen_schema_expr(inner);
            quote! { Expr::Eq(Box::new(#inner_e), Box::new(Expr::None)) }
        }
        ExprDef::ForAll {
            binding,
            over,
            body,
        } => {
            let b = binding.to_string();
            let over_e = gen_schema_expr(over);
            let body_e = gen_schema_expr(body);
            quote! { Expr::Quantified {
                quantifier: Quantifier::All,
                binding: #b.into(),
                over: Box::new(#over_e),
                body: Box::new(#body_e),
            } }
        }
        ExprDef::Exists {
            binding,
            over,
            body,
        } => {
            let b = binding.to_string();
            let over_e = gen_schema_expr(over);
            let body_e = gen_schema_expr(body);
            quote! { Expr::Quantified {
                quantifier: Quantifier::Any,
                binding: #b.into(),
                over: Box::new(#over_e),
                body: Box::new(#body_e),
            } }
        }
        ExprDef::Call { helper, args } => {
            let h = helper.to_string();
            let arg_exprs: Vec<_> = args.iter().map(gen_schema_expr).collect();
            quote! { Expr::Call { helper: #h.into(), args: vec![#(#arg_exprs),*] } }
        }
        ExprDef::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            let cond = gen_schema_expr(condition);
            let then_e = gen_schema_expr(then_expr);
            let else_e = gen_schema_expr(else_expr);
            quote! { Expr::IfElse {
                condition: Box::new(#cond),
                then_expr: Box::new(#then_e),
                else_expr: Box::new(#else_e),
            } }
        }
    }
}

fn gen_enum_variants(enum_def: &crate::ast::EnumDef) -> Vec<TokenStream> {
    enum_def
        .variants
        .iter()
        .map(|v| {
            let name = v.name.to_string();
            if v.fields.is_empty() {
                quote! { VariantSchema { name: #name.into(), fields: vec![] } }
            } else {
                let fields: Vec<_> = v
                    .fields
                    .iter()
                    .map(|f| {
                        let fname = f.name.to_string();
                        let fty = gen_type_ref(&f.ty);
                        quote! { FieldSchema { name: #fname.into(), ty: #fty } }
                    })
                    .collect();
                quote! { VariantSchema { name: #name.into(), fields: vec![#(#fields),*] } }
            }
        })
        .collect()
}

fn gen_helpers(def: &MachineDef) -> Vec<TokenStream> {
    def.helpers
        .iter()
        .map(|h| {
            let name = h.name.to_string();
            let return_ty = gen_type_ref(&h.return_ty);
            let body = gen_schema_expr_for(def, &h.body);
            let params: Vec<_> = h
                .params
                .iter()
                .map(|p| {
                    let pname = p.name.to_string();
                    let pty = gen_type_ref(&p.ty);
                    quote! { FieldSchema { name: #pname.into(), ty: #pty } }
                })
                .collect();
            quote! {
                HelperSchema {
                    name: #name.into(),
                    params: vec![#(#params),*],
                    returns: #return_ty,
                    body: #body,
                }
            }
        })
        .collect()
}

fn gen_invariants(def: &MachineDef) -> Vec<TokenStream> {
    def.invariants
        .iter()
        .map(|inv| {
            let name = inv.name.to_string();
            let expr = gen_schema_expr_for(def, &inv.expr);
            quote! {
                InvariantSchema { name: #name.into(), expr: #expr }
            }
        })
        .collect()
}

fn gen_transitions(def: &MachineDef) -> Vec<TokenStream> {
    def.transitions
        .iter()
        .map(|t| {
            let name = t.name.to_string();
            let to = t.to_phase.to_string();
            let variant = t.trigger.variant.to_string();
            let kind = match t.trigger.kind {
                crate::ast::TriggerKindDef::Input => quote! { TriggerKind::Input },
                crate::ast::TriggerKindDef::Signal => quote! { TriggerKind::Signal },
            };
            let bindings: Vec<_> = t
                .trigger
                .bindings
                .iter()
                .map(std::string::ToString::to_string)
                .collect();

            // Guards — strip phase-field references (they're in `from` instead)
            let guard_items: Vec<_> = t
                .guards
                .iter()
                .filter_map(|g| {
                    let stripped = strip_phase_guards(def, &g.expr);
                    stripped.map(|remaining| {
                        let expr = gen_schema_expr_for(def, &remaining);
                        let guard_name_str = &g.name;
                        quote! { Guard { name: #guard_name_str.into(), expr: #expr } }
                    })
                })
                .collect();
            let guards = quote! { vec![#(#guard_items),*] };

            // Updates
            let updates = gen_schema_updates(&t.updates);

            // Effects
            let effects: Vec<_> = t
                .effects
                .iter()
                .map(|e| {
                    let evariant = e.variant.to_string();
                    let fields: Vec<_> = e
                        .fields
                        .iter()
                        .map(|(fname, fval)| {
                            let fn_str = fname.to_string();
                            let val = gen_schema_expr_for(def, fval);
                            quote! { (#fn_str.into(), #val) }
                        })
                        .collect();
                    quote! {
                        EffectEmit {
                            variant: #evariant.into(),
                            fields: indexmap::IndexMap::from([#(#fields),*]),
                        }
                    }
                })
                .collect();

            // Derive `from` phases from guard expressions. `validate` has
            // already refused to compile the machine if any transition's
            // guard is unanalyzable; this branch reflects that invariant
            // via a `compile_error!` belt-and-suspenders for defense in
            // depth if someone bypasses validate in the future.
            match derive_from_phases(def, t) {
                Ok(from_phases) => quote! {
                    TransitionSchema {
                        name: #name.into(),
                        from: vec![#(#from_phases.into()),*],
                        on: InputMatch {
                            kind: #kind,
                            variant: #variant.into(),
                            bindings: vec![#(#bindings.into()),*],
                        },
                        guards: #guards,
                        updates: vec![#(#updates),*],
                        to: #to.into(),
                        emit: vec![#(#effects),*],
                    }
                },
                Err(msg) => {
                    let err = format!(
                        "internal error: from-phase derivation failed at schema codegen \
                         (validation should have caught this): {msg}"
                    );
                    quote! { compile_error!(#err) }
                }
            }
        })
        .collect()
}

fn gen_schema_updates(updates: &[crate::ast::UpdateDef]) -> Vec<TokenStream> {
    updates
        .iter()
        .map(|u| {
            match u {
                UpdateDef::Assign { field, value } => {
                    let f = field.to_string();
                    let v = gen_schema_expr(value);
                    quote! { Update::Assign { field: #f.into(), expr: #v } }
                }
                UpdateDef::Increment { field, amount } => {
                    let f = field.to_string();
                    // `validate::validate_arithmetic_amount` has already rejected
                    // non-literal amounts. If one slips through, emit a
                    // `compile_error!` rather than silently coerce to 1.
                    match amount {
                        crate::ast::ExprDef::U64(v) => {
                            quote! { Update::Increment { field: #f.into(), amount: #v } }
                        }
                        _ => non_literal_amount_compile_error(&f, "Increment"),
                    }
                }
                UpdateDef::Decrement { field, amount } => {
                    let f = field.to_string();
                    match amount {
                        crate::ast::ExprDef::U64(v) => {
                            quote! { Update::Decrement { field: #f.into(), amount: #v } }
                        }
                        _ => non_literal_amount_compile_error(&f, "Decrement"),
                    }
                }
                UpdateDef::SetInsert { field, value } => {
                    let f = field.to_string();
                    let v = gen_schema_expr(value);
                    quote! { Update::SetInsert { field: #f.into(), value: #v } }
                }
                UpdateDef::SetRemove { field, value } => {
                    let f = field.to_string();
                    let v = gen_schema_expr(value);
                    quote! { Update::SetRemove { field: #f.into(), value: #v } }
                }
                UpdateDef::MapInsert { field, key, value } => {
                    let f = field.to_string();
                    let k = gen_schema_expr(key);
                    let v = gen_schema_expr(value);
                    quote! { Update::MapInsert { field: #f.into(), key: #k, value: #v } }
                }
                UpdateDef::MapRemove { field, key } => {
                    let f = field.to_string();
                    let k = gen_schema_expr(key);
                    quote! { Update::MapRemove { field: #f.into(), key: #k } }
                }
                UpdateDef::MapIncrement { field, key, amount } => {
                    let f = field.to_string();
                    let k = gen_schema_expr(key);
                    match amount {
                        crate::ast::ExprDef::U64(v) => {
                            quote! { Update::MapIncrement { field: #f.into(), key: #k, amount: #v } }
                        }
                        _ => non_literal_amount_compile_error(&f, "MapIncrement"),
                    }
                }
                UpdateDef::MapDecrement { field, key, amount } => {
                    let f = field.to_string();
                    let k = gen_schema_expr(key);
                    match amount {
                        crate::ast::ExprDef::U64(v) => {
                            quote! { Update::MapDecrement { field: #f.into(), key: #k, amount: #v } }
                        }
                        _ => non_literal_amount_compile_error(&f, "MapDecrement"),
                    }
                }
                UpdateDef::Conditional {
                    condition,
                    then_updates,
                    else_updates,
                } => {
                    let cond = gen_schema_expr(condition);
                    let then_u = gen_schema_updates(then_updates);
                    let else_u = gen_schema_updates(else_updates);
                    quote! { Update::Conditional {
                        condition: #cond,
                        then_updates: vec![#(#then_u),*],
                        else_updates: vec![#(#else_u),*],
                    } }
                }
            }
        })
        .collect()
}

/// Compositional representation of the phase-set a guard constrains to.
///
/// The lattice:
/// - `All` = "no phase constraint from this guard" (e.g., non-phase guards,
///   or absent guards).
/// - `Only(S)` = "transition is reachable from exactly the phases in S".
/// - `Unanalyzable(msg)` = "the guard references the phase field in a shape
///   the compiler can't lower to a precise phase set". Surfaced as a
///   compile-time error rather than silently coerced to `All`.
#[derive(Debug, Clone)]
pub(crate) enum PhaseSet {
    All,
    Only(Vec<String>),
    Unanalyzable(String),
}

impl PhaseSet {
    fn intersect(self, other: PhaseSet, all_phases: &[String]) -> PhaseSet {
        match (self, other) {
            (PhaseSet::Unanalyzable(m), _) | (_, PhaseSet::Unanalyzable(m)) => {
                PhaseSet::Unanalyzable(m)
            }
            (PhaseSet::All, rhs) => rhs,
            (lhs, PhaseSet::All) => lhs,
            (PhaseSet::Only(a), PhaseSet::Only(b)) => {
                let set: Vec<String> = a.into_iter().filter(|p| b.iter().any(|q| q == p)).collect();
                if set.is_empty() {
                    // Intersection is empty — transition is unreachable.
                    // This is either a machine-author bug (A && B with
                    // incompatible phase constraints) or an over-eager
                    // analysis. Surface as an error.
                    PhaseSet::Unanalyzable(format!(
                        "guard conjunction narrows to empty phase set (all phases: {all_phases:?})"
                    ))
                } else {
                    PhaseSet::Only(set)
                }
            }
        }
    }

    fn union(self, other: PhaseSet) -> PhaseSet {
        match (self, other) {
            (PhaseSet::Unanalyzable(m), _) | (_, PhaseSet::Unanalyzable(m)) => {
                PhaseSet::Unanalyzable(m)
            }
            (PhaseSet::All, _) | (_, PhaseSet::All) => PhaseSet::All,
            (PhaseSet::Only(mut a), PhaseSet::Only(b)) => {
                for p in b {
                    if !a.contains(&p) {
                        a.push(p);
                    }
                }
                PhaseSet::Only(a)
            }
        }
    }

    fn complement(self, all_phases: &[String]) -> PhaseSet {
        match self {
            PhaseSet::All => {
                // Negating "no constraint" gives "no constraint" back —
                // we can't derive a useful from-set. That's legitimate
                // (e.g., `guard { !self.active }`), so leave as All.
                PhaseSet::All
            }
            PhaseSet::Only(s) => {
                let rest: Vec<String> = all_phases
                    .iter()
                    .filter(|p| !s.contains(p))
                    .cloned()
                    .collect();
                if rest.is_empty() {
                    PhaseSet::Unanalyzable(
                        "negation of full phase set yields empty from-set".into(),
                    )
                } else {
                    PhaseSet::Only(rest)
                }
            }
            PhaseSet::Unanalyzable(m) => PhaseSet::Unanalyzable(m),
        }
    }
}

/// Compute the `PhaseSet` for a guard expression. For stored-phase machines,
/// walks the expression and extracts phase constraints compositionally.
///
/// Pattern-matching is exhaustive over the shapes this compiler claims to
/// understand. Anything else that *references the phase field* is reported
/// as `Unanalyzable` so the caller can raise a compile error — never
/// silently coerced to `All` (the historical behavior, which over-permitted
/// the TLA+ model).
pub(crate) fn compute_phase_set_stored(def: &MachineDef, expr: &ExprDef) -> PhaseSet {
    use crate::ast::ExprDef;

    let phase_field_name = match def.phase_field_name() {
        Some(f) => f.to_string(),
        None => {
            // Should not happen — only called on stored-phase machines.
            return PhaseSet::Unanalyzable(
                "compute_phase_set_stored called on non-stored-phase machine".into(),
            );
        }
    };

    let all_phases: Vec<String> = def
        .phase_enum
        .variants
        .iter()
        .map(std::string::ToString::to_string)
        .collect();

    // If the expression doesn't reference the phase field at all, it can't
    // constrain the phase set. Return All (legitimate non-phase guard).
    if !references_phase_field(expr, &phase_field_name) {
        return PhaseSet::All;
    }

    match expr {
        // self.lifecycle_phase == Phase::Draft  (or the mirror orientation)
        ExprDef::Eq(left, right) => {
            if let Some(phase) = phase_eq_literal(left, right, &phase_field_name) {
                PhaseSet::Only(vec![phase])
            } else {
                PhaseSet::Unanalyzable(format!(
                    "guard `==` with phase field `{phase_field_name}` must compare against a \
                     `Phase::Variant` literal (one side phase field, other side phase literal)"
                ))
            }
        }
        // self.lifecycle_phase != Phase::Draft  → all_phases \ {Draft}
        ExprDef::Neq(left, right) => {
            if let Some(phase) = phase_eq_literal(left, right, &phase_field_name) {
                PhaseSet::Only(
                    all_phases
                        .iter()
                        .filter(|p| *p != &phase)
                        .cloned()
                        .collect(),
                )
            } else {
                PhaseSet::Unanalyzable(format!(
                    "guard `!=` with phase field `{phase_field_name}` must compare against a \
                     `Phase::Variant` literal"
                ))
            }
        }
        // helper(self.lifecycle_phase, ...) — expand helper body
        ExprDef::Call { helper, args } => {
            let has_phase_arg = args
                .iter()
                .any(|a| is_phase_field_expr(a, &phase_field_name));
            if !has_phase_arg {
                // Helper call that doesn't take the phase field can still
                // reference it indirectly via state-field closures, but our
                // helpers are pure functions of parameters, so this means
                // the guard references the phase field elsewhere. Fall
                // through to an error.
                return PhaseSet::Unanalyzable(format!(
                    "helper `{helper}` does not take the phase field — cannot derive from-set"
                ));
            }
            match def.helpers.iter().find(|h| h.name == *helper) {
                Some(h) => extract_phase_set_from_helper_body(&h.body, &all_phases, helper),
                None => PhaseSet::Unanalyzable(format!("unknown helper `{helper}` in guard")),
            }
        }
        // guard1 && guard2 — intersection of phase sets
        ExprDef::And(exprs) => exprs
            .iter()
            .map(|e| compute_phase_set_stored(def, e))
            .fold(PhaseSet::All, |acc, p| acc.intersect(p, &all_phases)),
        // guard1 || guard2 — union of phase sets
        ExprDef::Or(exprs) => exprs
            .iter()
            .map(|e| compute_phase_set_stored(def, e))
            .fold(PhaseSet::Only(Vec::new()), PhaseSet::union),
        // !guard — complement (only when the inner set is concrete)
        ExprDef::Not(inner) => compute_phase_set_stored(def, inner).complement(&all_phases),
        // Any other expression that references the phase field is
        // something this compiler doesn't claim to analyze yet. Refuse
        // to emit a schema for it rather than silently default to All.
        _ => PhaseSet::Unanalyzable(format!(
            "guard shape referencing phase field `{phase_field_name}` is not yet supported by \
             schema from-phase derivation (extend `compute_phase_set_stored` to cover it)"
        )),
    }
}

fn references_phase_field(expr: &ExprDef, phase_field_name: &str) -> bool {
    match expr {
        ExprDef::Field(name) => name == phase_field_name,
        ExprDef::CurrentPhase => true,
        ExprDef::Not(inner)
        | ExprDef::IsSome(inner)
        | ExprDef::IsNone(inner)
        | ExprDef::Len(inner)
        | ExprDef::MapKeys(inner)
        | ExprDef::Some(inner) => references_phase_field(inner, phase_field_name),
        ExprDef::And(exprs) | ExprDef::Or(exprs) => exprs
            .iter()
            .any(|e| references_phase_field(e, phase_field_name)),
        ExprDef::Eq(l, r)
        | ExprDef::Neq(l, r)
        | ExprDef::Gt(l, r)
        | ExprDef::Gte(l, r)
        | ExprDef::Lt(l, r)
        | ExprDef::Lte(l, r)
        | ExprDef::Add(l, r)
        | ExprDef::Sub(l, r) => {
            references_phase_field(l, phase_field_name)
                || references_phase_field(r, phase_field_name)
        }
        ExprDef::Contains { collection, value }
        | ExprDef::MapContainsKey {
            map: collection,
            key: value,
        }
        | ExprDef::MapGet {
            map: collection,
            key: value,
        }
        | ExprDef::MapGetCopied {
            map: collection,
            key: value,
        } => {
            references_phase_field(collection, phase_field_name)
                || references_phase_field(value, phase_field_name)
        }
        ExprDef::Call { args, .. } => args
            .iter()
            .any(|a| references_phase_field(a, phase_field_name)),
        ExprDef::ForAll { over, body, .. } | ExprDef::Exists { over, body, .. } => {
            references_phase_field(over, phase_field_name)
                || references_phase_field(body, phase_field_name)
        }
        ExprDef::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            references_phase_field(condition, phase_field_name)
                || references_phase_field(then_expr, phase_field_name)
                || references_phase_field(else_expr, phase_field_name)
        }
        _ => false,
    }
}

/// If `left` and `right` form `self.<phase_field> == Phase::X` (or mirrored),
/// return `Some("X")`. Otherwise `None`. Requires one side to reference the
/// phase field and the other to be a `Phase::X` literal.
fn phase_eq_literal(left: &ExprDef, right: &ExprDef, phase_field_name: &str) -> Option<String> {
    let has_phase_field =
        is_phase_field(left, phase_field_name) || is_phase_field(right, phase_field_name);
    if has_phase_field {
        phase_variant_either_side(left, right)
    } else {
        None
    }
}

/// Extract a `PhaseSet` from a helper body. Helpers are pure functions of
/// their parameters, so the body typically looks like
/// `param == Phase::A || param == Phase::B || ...`. Anything else yields
/// `Unanalyzable`.
fn extract_phase_set_from_helper_body(
    expr: &ExprDef,
    all_phases: &[String],
    helper_name: &syn::Ident,
) -> PhaseSet {
    let unsupported = || {
        PhaseSet::Unanalyzable(format!(
            "helper `{helper_name}` body must be a disjunction of \
             `param == Phase::X` equalities"
        ))
    };

    match expr {
        // p == Phase::X  or  Phase::X == p
        ExprDef::Eq(left, right) => match phase_variant_either_side(left, right) {
            Some(variant) => PhaseSet::Only(vec![variant]),
            None => unsupported(),
        },
        // p != Phase::X  →  complement
        ExprDef::Neq(left, right) => match phase_variant_either_side(left, right) {
            Some(variant) => PhaseSet::Only(
                all_phases
                    .iter()
                    .filter(|p| *p != &variant)
                    .cloned()
                    .collect(),
            ),
            None => unsupported(),
        },
        // p == Phase::X || p == Phase::Y || ...
        ExprDef::Or(exprs) => exprs
            .iter()
            .map(|e| extract_phase_set_from_helper_body(e, all_phases, helper_name))
            .fold(PhaseSet::Only(Vec::new()), PhaseSet::union),
        // p == Phase::X && p != Phase::Y (rare but valid)
        ExprDef::And(exprs) => exprs
            .iter()
            .map(|e| extract_phase_set_from_helper_body(e, all_phases, helper_name))
            .fold(PhaseSet::All, |acc, p| acc.intersect(p, all_phases)),
        _ => PhaseSet::Unanalyzable(format!(
            "helper `{helper_name}` body shape not supported for phase-set extraction — \
             use a disjunction of `param == Phase::X` equalities"
        )),
    }
}

/// If either side is a `Phase::X` literal, return `Some("X")`.
fn phase_variant_either_side(left: &ExprDef, right: &ExprDef) -> Option<String> {
    if let ExprDef::Phase(v) = right {
        return Some(v.to_string());
    }
    if let ExprDef::Phase(v) = left {
        return Some(v.to_string());
    }
    None
}

/// Derive `from` phases for a transition from its guard expression.
///
/// Returns a `Result` so the caller can emit a span-attached compile error
/// if the guard shape prevents the compiler from producing a precise
/// from-set. Historically this function returned `Vec<String>` and fell
/// back to `all_phases` on any unanalyzable guard, weakening the TLA+
/// model; that fallback is gone.
///
/// For stored-phase machines: compositional `PhaseSet` extraction over the
/// combined guard. For derived-phase machines: per-phase satisfiability
/// check against the phase projection's field facts.
pub(crate) fn derive_from_phases(
    def: &MachineDef,
    t: &TransitionDef,
) -> Result<Vec<String>, String> {
    // All phases are candidates for `from` — including terminal phases.
    // A transition CAN originate from a terminal phase (the schema records where
    // transitions are reachable, not where they're useful).
    let all_phases: Vec<String> = def
        .phase_enum
        .variants
        .iter()
        .map(std::string::ToString::to_string)
        .collect();

    let combined = match t.combined_guard() {
        Some(g) => g,
        None => return Ok(all_phases), // no guards → all phases (legitimate)
    };

    if def.is_stored_phase() {
        match compute_phase_set_stored(def, &combined) {
            PhaseSet::All => Ok(all_phases),
            PhaseSet::Only(set) if set.is_empty() => Err(format!(
                "transition `{}` derives to an empty phase set (unreachable)",
                t.name
            )),
            PhaseSet::Only(set) => Ok(set),
            PhaseSet::Unanalyzable(msg) => Err(format!("transition `{}`: {msg}", t.name)),
        }
    } else {
        // Derived-phase: enumerate all phases, substitute the phase
        // projection's defining conditions into the guard, check if
        // trivially false.
        derive_from_phases_derived(def, &combined, &all_phases, &t.name)
    }
}

/// For derived-phase machines: determine which phases are compatible with a guard.
///
/// For each phase, the projection defines conditions under which that phase is active.
/// We check if the guard is satisfiable given those conditions.
///
/// Algorithm: for each candidate phase, collect the field constraints implied by
/// the phase projection (the phase's own condition AND the negation of all
/// higher-priority phases' conditions). Then check if the guard is trivially
/// contradicted by those constraints.
fn derive_from_phases_derived(
    def: &MachineDef,
    guard: &ExprDef,
    all_phases: &[String],
    transition_name: &syn::Ident,
) -> Result<Vec<String>, String> {
    let proj = match &def.phase_projection {
        Some(p) => p,
        None => {
            return Err(format!(
                "transition `{transition_name}`: derived-phase machine lacks a \
                 phase_projection block — cannot derive from-set"
            ));
        }
    };

    // Collect the field constraints that the guard references.
    let guard_fields = collect_field_refs(guard);
    if guard_fields.is_empty() {
        return Ok(all_phases.to_vec());
    }

    let mut from = Vec::new();

    for phase_name in all_phases {
        // Find the projection rule for this phase
        let rule_idx = proj
            .rules
            .iter()
            .position(|r| r.phase == phase_name.as_str());
        let rule_idx = match rule_idx {
            Some(i) => i,
            None => {
                from.push(phase_name.clone());
                continue;
            }
        };

        // Build the field facts for this phase:
        // - All higher-priority rules' conditions are FALSE
        // - This rule's condition is TRUE (if it has one)
        let mut field_facts: Vec<(String, FieldFact)> = Vec::new();

        for (i, rule) in proj.rules.iter().enumerate() {
            if i < rule_idx {
                // Higher-priority: its condition is FALSE for our phase
                if let Some(cond) = &rule.condition {
                    collect_negated_facts(cond, &mut field_facts);
                }
            } else if i == rule_idx {
                // Our phase: its condition is TRUE
                if let Some(cond) = &rule.condition {
                    collect_positive_facts(cond, &mut field_facts);
                }
            }
        }

        // Check if the guard is contradicted by these facts
        if !is_contradicted(guard, &field_facts) {
            from.push(phase_name.clone());
        }
    }

    if from.is_empty() {
        // This used to silently fall back to `all_phases`. That hid real
        // bugs (the machine declares a guard that's unsatisfiable against
        // every phase). Surface it as an error — the machine author should
        // either fix the guard, add a phase to the projection, or tighten
        // the projection's field conditions.
        Err(format!(
            "transition `{transition_name}`: guard is contradicted against every phase — \
             no reachable from-phase (check projection conditions and guard field facts)"
        ))
    } else {
        Ok(from)
    }
}

/// A fact about a field's value, derived from phase projection conditions.
#[derive(Debug)]
#[allow(dead_code)] // variants used for future derived-phase analysis
enum FieldFact {
    IsTrue,
    IsFalse,
    IsSome,
    IsNone,
    GteValue(u64),
    LtValue(u64),
    GtZero,
    EqZero,
}

/// Collect field references from an expression.
fn collect_field_refs(expr: &ExprDef) -> Vec<String> {
    let mut refs = Vec::new();
    collect_field_refs_inner(expr, &mut refs);
    refs
}

fn collect_field_refs_inner(expr: &ExprDef, out: &mut Vec<String>) {
    match expr {
        ExprDef::Field(name) => out.push(name.to_string()),
        ExprDef::Not(inner) => collect_field_refs_inner(inner, out),
        ExprDef::And(exprs) | ExprDef::Or(exprs) => {
            for e in exprs {
                collect_field_refs_inner(e, out);
            }
        }
        ExprDef::Eq(l, r)
        | ExprDef::Neq(l, r)
        | ExprDef::Gt(l, r)
        | ExprDef::Gte(l, r)
        | ExprDef::Lt(l, r)
        | ExprDef::Lte(l, r)
        | ExprDef::Add(l, r)
        | ExprDef::Sub(l, r) => {
            collect_field_refs_inner(l, out);
            collect_field_refs_inner(r, out);
        }
        ExprDef::IsSome(inner) | ExprDef::IsNone(inner) => collect_field_refs_inner(inner, out),
        ExprDef::Contains { collection, value }
        | ExprDef::MapContainsKey {
            map: collection,
            key: value,
        } => {
            collect_field_refs_inner(collection, out);
            collect_field_refs_inner(value, out);
        }
        _ => {}
    }
}

/// Extract positive facts from a condition (the condition is TRUE).
fn collect_positive_facts(expr: &ExprDef, facts: &mut Vec<(String, FieldFact)>) {
    match expr {
        // self.active → active is true
        ExprDef::Field(name) => facts.push((name.to_string(), FieldFact::IsTrue)),
        // !self.active → active is false
        ExprDef::Not(inner) => {
            if let ExprDef::Field(name) = inner.as_ref() {
                facts.push((name.to_string(), FieldFact::IsFalse));
            }
        }
        // self.field.is_some() → field is Some
        ExprDef::IsSome(inner) => {
            if let ExprDef::Field(name) = inner.as_ref() {
                facts.push((name.to_string(), FieldFact::IsSome));
            }
        }
        // self.field.is_none() → field is None
        ExprDef::IsNone(inner) => {
            if let ExprDef::Field(name) = inner.as_ref() {
                facts.push((name.to_string(), FieldFact::IsNone));
            }
        }
        // self.value >= self.limit → value >= limit (simplified: value is GteValue)
        ExprDef::Gte(left, _right) => {
            if let ExprDef::Field(name) = left.as_ref() {
                facts.push((name.to_string(), FieldFact::GtZero));
            }
        }
        // self.value > 0 → value is GtZero
        ExprDef::Gt(left, right) => {
            if let (ExprDef::Field(name), ExprDef::U64(0)) = (left.as_ref(), right.as_ref()) {
                facts.push((name.to_string(), FieldFact::GtZero));
            }
        }
        // conjunction: extract from both sides
        ExprDef::And(exprs) => {
            for e in exprs {
                collect_positive_facts(e, facts);
            }
        }
        _ => {}
    }
}

/// Extract negated facts from a condition (the condition is FALSE).
fn collect_negated_facts(expr: &ExprDef, facts: &mut Vec<(String, FieldFact)>) {
    match expr {
        // condition `self.active` is FALSE → active is false
        ExprDef::Field(name) => facts.push((name.to_string(), FieldFact::IsFalse)),
        // condition `!self.active` is FALSE → active is true
        ExprDef::Not(inner) => {
            if let ExprDef::Field(name) = inner.as_ref() {
                facts.push((name.to_string(), FieldFact::IsTrue));
            }
        }
        // condition `self.field.is_some()` is FALSE → field is None
        ExprDef::IsSome(inner) => {
            if let ExprDef::Field(name) = inner.as_ref() {
                facts.push((name.to_string(), FieldFact::IsNone));
            }
        }
        // condition `self.value > 0` is FALSE → value == 0
        ExprDef::Gt(left, right) => {
            if let (ExprDef::Field(name), ExprDef::U64(0)) = (left.as_ref(), right.as_ref()) {
                facts.push((name.to_string(), FieldFact::EqZero));
            }
        }
        // For OR conditions being FALSE, ALL sub-conditions are false
        ExprDef::Or(exprs) => {
            for e in exprs {
                collect_negated_facts(e, facts);
            }
        }
        _ => {}
    }
}

/// Check if a guard expression is contradicted by known field facts.
fn is_contradicted(guard: &ExprDef, facts: &[(String, FieldFact)]) -> bool {
    match guard {
        // self.active && facts say active is false → contradicted
        ExprDef::Field(name) => facts
            .iter()
            .any(|(f, fact)| f == &name.to_string() && matches!(fact, FieldFact::IsFalse)),
        ExprDef::Not(inner) => {
            if let ExprDef::Field(name) = inner.as_ref() {
                facts
                    .iter()
                    .any(|(f, fact)| f == &name.to_string() && matches!(fact, FieldFact::IsTrue))
            } else {
                false
            }
        }
        ExprDef::IsSome(inner) => {
            if let ExprDef::Field(name) = inner.as_ref() {
                facts
                    .iter()
                    .any(|(f, fact)| f == &name.to_string() && matches!(fact, FieldFact::IsNone))
            } else {
                false
            }
        }
        ExprDef::IsNone(inner) => {
            if let ExprDef::Field(name) = inner.as_ref() {
                facts
                    .iter()
                    .any(|(f, fact)| f == &name.to_string() && matches!(fact, FieldFact::IsSome))
            } else {
                false
            }
        }
        ExprDef::Eq(left, right) => {
            // self.field == None but facts say field is Some → contradicted
            if let (ExprDef::Field(name), ExprDef::None) = (left.as_ref(), right.as_ref()) {
                return facts
                    .iter()
                    .any(|(f, fact)| f == &name.to_string() && matches!(fact, FieldFact::IsSome));
            }
            if let (ExprDef::None, ExprDef::Field(name)) = (left.as_ref(), right.as_ref()) {
                return facts
                    .iter()
                    .any(|(f, fact)| f == &name.to_string() && matches!(fact, FieldFact::IsSome));
            }
            false
        }
        // AND: if any conjunct is contradicted, the whole guard is contradicted
        ExprDef::And(exprs) => exprs.iter().any(|e| is_contradicted(e, facts)),
        // OR: only contradicted if ALL disjuncts are contradicted
        ExprDef::Or(exprs) => exprs.iter().all(|e| is_contradicted(e, facts)),
        _ => false,
    }
}

fn is_phase_field(expr: &ExprDef, phase_field_name: &str) -> bool {
    matches!(expr, ExprDef::Field(name) if name == phase_field_name)
        || matches!(expr, ExprDef::CurrentPhase)
}

fn is_phase_field_expr(expr: &ExprDef, phase_field_name: &str) -> bool {
    is_phase_field(expr, phase_field_name)
}

/// Strip phase-field references from a guard expression.
///
/// For stored-phase machines, guards like `self.lifecycle_phase == Phase::Pending`
/// are handled by the `from` list, not by schema guards. This function removes
/// those comparisons, returning `None` if the entire guard was phase-only.
fn strip_phase_guards(def: &MachineDef, expr: &ExprDef) -> Option<ExprDef> {
    if !def.is_stored_phase() {
        return Some(expr.clone());
    }

    match expr {
        // self.lifecycle_phase == Phase::X → remove entirely
        ExprDef::Eq(left, right) | ExprDef::Neq(left, right) => {
            if is_phase_ref(def, left) || is_phase_ref(def, right) {
                None
            } else {
                Some(expr.clone())
            }
        }
        // helper(self.lifecycle_phase) → remove entirely
        ExprDef::Call { args, .. } => {
            if args.iter().any(|a| is_phase_ref(def, a)) {
                None
            } else {
                Some(expr.clone())
            }
        }
        // conjunction: filter out phase predicates, keep the rest
        ExprDef::And(exprs) => {
            let remaining: Vec<_> = exprs
                .iter()
                .filter_map(|e| strip_phase_guards(def, e))
                .collect();
            match remaining.len() {
                0 => None,
                1 => remaining.into_iter().next(),
                _ => Some(ExprDef::And(remaining)),
            }
        }
        // disjunction: filter out phase predicates
        ExprDef::Or(exprs) => {
            let remaining: Vec<_> = exprs
                .iter()
                .filter_map(|e| strip_phase_guards(def, e))
                .collect();
            match remaining.len() {
                0 => None,
                1 => remaining.into_iter().next(),
                _ => Some(ExprDef::Or(remaining)),
            }
        }
        // Negation of a phase ref
        ExprDef::Not(inner) => {
            if strip_phase_guards(def, inner).is_none() {
                None
            } else {
                Some(expr.clone())
            }
        }
        _ => Some(expr.clone()),
    }
}

/// Check if an expression is a reference to the stored phase field.
fn is_phase_ref(def: &MachineDef, expr: &ExprDef) -> bool {
    match expr {
        ExprDef::Field(name) => def.phase_field_name() == Some(name),
        ExprDef::CurrentPhase => true,
        _ => false,
    }
}

fn gen_dispositions(def: &MachineDef) -> Vec<TokenStream> {
    def.dispositions.iter().map(|d| {
        let effect = d.effect.to_string();
        let kind = match &d.kind {
            crate::ast::DispositionKind::Local => quote! { EffectDisposition::Local },
            crate::ast::DispositionKind::External => quote! { EffectDisposition::External },
            crate::ast::DispositionKind::Routed(machines) => {
                let names: Vec<_> = machines.iter().map(std::string::ToString::to_string).collect();
                quote! { EffectDisposition::Routed { consumer_machines: vec![#(#names.into()),*] } }
            }
        };
        quote! {
            EffectDispositionRule {
                effect_variant: #effect.into(),
                disposition: #kind,
                handoff_protocol: None,
            }
        }
    }).collect()
}
