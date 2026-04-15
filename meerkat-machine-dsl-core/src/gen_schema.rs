use proc_macro2::TokenStream;
use quote::quote;

use crate::ast::MachineDef;

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
    let terminal_phases: Vec<_> = def.terminal_phases.iter().map(|p| p.to_string()).collect();

    let input_name = def.inputs.name.to_string();
    let input_variants = gen_enum_variants(&def.inputs);

    let surface_only: Vec<_> = def
        .surface_only_inputs
        .iter()
        .map(|i| i.to_string())
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

    quote! {
        impl #state_name {
            pub fn schema() -> meerkat_machine_schema::MachineSchema {
                use meerkat_machine_schema::*;

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
    }
}

fn gen_state_fields(def: &MachineDef) -> Vec<TokenStream> {
    def.state_fields
        .iter()
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

    // For stored-phase, emit the phase field init
    if def.is_stored_phase() {
        let phase_field_name = def.phase_field_name().unwrap().to_string();
        let init_phase = def.init_phase.to_string();
        fields.push(quote! {
            FieldInit { field: #phase_field_name.into(), expr: Expr::Phase(#init_phase.into()) }
        });
    }

    for init in &def.init_fields {
        let name = init.name.to_string();
        let expr = gen_schema_expr(&init.value);
        fields.push(quote! {
            FieldInit { field: #name.into(), expr: #expr }
        });
    }
    fields
}

fn gen_schema_expr(expr: &crate::ast::ExprDef) -> TokenStream {
    use crate::ast::ExprDef;
    match expr {
        ExprDef::Bool(v) => quote! { Expr::Bool(#v) },
        ExprDef::U64(v) => quote! { Expr::U64(#v) },
        ExprDef::StringLit(s) => quote! { Expr::String(#s.into()) },
        ExprDef::None => quote! { Expr::None },
        ExprDef::Some(inner) => {
            let inner_e = gen_schema_expr(inner);
            quote! { Expr::Some(Box::new(#inner_e)) }
        }
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
        ExprDef::Len(inner) => {
            let inner_e = gen_schema_expr(inner);
            quote! { Expr::Len(Box::new(#inner_e)) }
        }
        ExprDef::MapGet { map, key } => {
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
            let body = gen_schema_expr(&h.body);
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
            let expr = gen_schema_expr(&inv.expr);
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
            let bindings: Vec<_> = t.trigger.bindings.iter().map(|b| b.to_string()).collect();

            // Guards
            let guards = if let Some(guard) = &t.guard {
                let expr = gen_schema_expr(guard);
                quote! { vec![Guard { name: String::new(), expr: #expr }] }
            } else {
                quote! { vec![] }
            };

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
                            let val = gen_schema_expr(fval);
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

            // Derive `from` phases — for now emit empty and let validation fill in
            // (the full derivation algorithm is a later step)
            quote! {
                TransitionSchema {
                    name: #name.into(),
                    from: vec![], // derived by validation
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
            }
        })
        .collect()
}

fn gen_schema_updates(updates: &[crate::ast::UpdateDef]) -> Vec<TokenStream> {
    updates
        .iter()
        .map(|u| {
            use crate::ast::UpdateDef;
            match u {
                UpdateDef::Assign { field, value } => {
                    let f = field.to_string();
                    let v = gen_schema_expr(value);
                    quote! { Update::Assign { field: #f.into(), expr: #v } }
                }
                UpdateDef::Increment { field, amount } => {
                    let f = field.to_string();
                    let a = match amount {
                        crate::ast::ExprDef::U64(v) => *v,
                        _ => 1, // default increment
                    };
                    quote! { Update::Increment { field: #f.into(), amount: #a } }
                }
                UpdateDef::Decrement { field, amount } => {
                    let f = field.to_string();
                    let a = match amount {
                        crate::ast::ExprDef::U64(v) => *v,
                        _ => 1,
                    };
                    quote! { Update::Decrement { field: #f.into(), amount: #a } }
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

fn gen_dispositions(def: &MachineDef) -> Vec<TokenStream> {
    def.dispositions.iter().map(|d| {
        let effect = d.effect.to_string();
        let kind = match &d.kind {
            crate::ast::DispositionKind::Local => quote! { EffectDisposition::Local },
            crate::ast::DispositionKind::External => quote! { EffectDisposition::External },
            crate::ast::DispositionKind::Routed(machines) => {
                let names: Vec<_> = machines.iter().map(|m| m.to_string()).collect();
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
