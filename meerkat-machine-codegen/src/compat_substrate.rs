#![allow(dead_code)]

use meerkat_machine_dsl_core::{
    ast::{
        DispositionDef, DispositionKind, EffectEmitDef, EnumDef, ExprDef, FieldDef, GuardDef,
        HelperDef, InitFieldDef, InvariantDef, MachineDef, PhaseEnumDef, TransitionDef, TriggerDef,
        TriggerKindDef, TypeDef, UpdateDef, VariantDef,
    },
    generate_from_def,
};
use meerkat_machine_schema::{
    EffectDisposition, EffectDispositionRule, EffectEmit, Expr, FieldSchema, Guard, HelperSchema,
    InvariantSchema, MachineSchema, Quantifier, TransitionSchema, TriggerKind, TypeRef, Update,
    VariantSchema,
};
use proc_macro2::Span;
use syn::Ident;

pub fn render_substrate(schema: &MachineSchema) -> String {
    let mut def = machine_def_from_schema(schema);
    match generate_from_def(&mut def) {
        Ok(tokens) => tokens.to_string(),
        Err(error) => {
            let message = format!("compat substrate generation should succeed: {error}");
            format!("compile_error!({message:?});")
        }
    }
}

fn machine_def_from_schema(schema: &MachineSchema) -> MachineDef {
    MachineDef {
        name: ident("CompatMachine"),
        version: schema.version,
        rust_crate: "self".into(),
        rust_module: "generated::compat_substrate".into(),
        state_fields: state_fields(schema),
        init_phase: ident(&schema.state.init.phase),
        init_fields: init_fields(schema),
        terminal_phases: schema
            .state
            .terminal_phases
            .iter()
            .map(|phase| ident(phase))
            .collect(),
        phase_enum: PhaseEnumDef {
            name: ident("CompatPhase"),
            variants: schema
                .state
                .phase
                .variants
                .iter()
                .map(|variant| ident(&variant.name))
                .collect(),
        },
        phase_projection: None,
        inputs: EnumDef {
            name: ident("CompatInput"),
            variants: variants(&schema.inputs.variants),
        },
        surface_only_inputs: schema
            .surface_only_inputs
            .iter()
            .map(|name| ident(name))
            .collect(),
        signals: EnumDef {
            name: ident("CompatSignal"),
            variants: variants(&schema.signals.variants),
        },
        effects: EnumDef {
            name: ident("CompatEffect"),
            variants: variants(&schema.effects.variants),
        },
        helpers: schema
            .helpers
            .iter()
            .chain(schema.derived.iter())
            .map(helper)
            .collect(),
        invariants: schema.invariants.iter().map(invariant).collect(),
        transitions: schema
            .transitions
            .iter()
            .map(|transition| transition_from_schema(schema, transition))
            .collect(),
        dispositions: schema.effect_dispositions.iter().map(disposition).collect(),
    }
}

fn state_fields(schema: &MachineSchema) -> Vec<FieldDef> {
    std::iter::once(FieldDef {
        name: ident("phase"),
        ty: TypeDef::Enum(ident("CompatPhase")),
        span: Span::call_site(),
    })
    .chain(schema.state.fields.iter().map(field))
    .collect()
}

fn init_fields(schema: &MachineSchema) -> Vec<InitFieldDef> {
    schema
        .state
        .init
        .fields
        .iter()
        .map(|field_init| InitFieldDef {
            name: ident(&field_init.field),
            value: expr_with_target(
                &field_init.expr,
                field_type(schema, &field_init.field),
                true,
            ),
            span: Span::call_site(),
        })
        .collect()
}

fn variants(variants: &[VariantSchema]) -> Vec<VariantDef> {
    variants
        .iter()
        .map(|variant| VariantDef {
            name: ident(&variant.name),
            fields: variant.fields.iter().map(field).collect(),
        })
        .collect()
}

fn field(field: &FieldSchema) -> FieldDef {
    FieldDef {
        name: ident(&field.name),
        ty: type_def(&field.ty),
        span: Span::call_site(),
    }
}

fn type_def(type_ref: &TypeRef) -> TypeDef {
    match type_ref {
        TypeRef::Bool => TypeDef::Bool,
        TypeRef::U32 => TypeDef::U32,
        TypeRef::U64 => TypeDef::U64,
        TypeRef::String => TypeDef::String,
        TypeRef::Named(name) => TypeDef::Named(ident(name)),
        TypeRef::Enum(name) => TypeDef::Enum(ident(name)),
        TypeRef::Option(inner) => TypeDef::Option(Box::new(type_def(inner))),
        TypeRef::Set(inner) => TypeDef::Set(Box::new(type_def(inner))),
        TypeRef::Seq(inner) => TypeDef::Seq(Box::new(type_def(inner))),
        TypeRef::Map(key, value) => {
            TypeDef::Map(Box::new(type_def(key)), Box::new(type_def(value)))
        }
    }
}

fn helper(helper: &HelperSchema) -> HelperDef {
    HelperDef {
        name: ident(&helper.name),
        params: helper.params.iter().map(field).collect(),
        return_ty: type_def(&helper.returns),
        body: expr_from_schema(&helper.body, false),
    }
}

fn invariant(invariant: &InvariantSchema) -> InvariantDef {
    InvariantDef {
        name: ident(&invariant.name),
        expr: expr_from_schema(&invariant.expr, false),
    }
}

fn transition_from_schema(schema: &MachineSchema, transition: &TransitionSchema) -> TransitionDef {
    TransitionDef {
        name: ident(&transition.name),
        per_phase: None,
        trigger: TriggerDef {
            kind: match transition.on.kind {
                TriggerKind::Input => TriggerKindDef::Input,
                TriggerKind::Signal => TriggerKindDef::Signal,
            },
            variant: ident(&transition.on.variant),
            bindings: transition
                .on
                .bindings
                .iter()
                .map(|binding| ident(binding))
                .collect(),
        },
        guards: transition.guards.iter().map(guard).collect(),
        updates: transition
            .updates
            .iter()
            .map(|update| update_from_schema_with_context(schema, update))
            .collect(),
        to_phase: ident(&transition.to),
        effects: transition.emit.iter().map(effect_emit).collect(),
        span: Span::call_site(),
    }
}

fn guard(guard: &Guard) -> GuardDef {
    GuardDef {
        name: guard.name.clone(),
        expr: expr_from_schema(&guard.expr, false),
    }
}

fn effect_emit(effect: &EffectEmit) -> EffectEmitDef {
    EffectEmitDef {
        variant: ident(&effect.variant),
        fields: effect
            .fields
            .iter()
            .map(|(name, value)| (ident(name), expr_from_schema(value, false)))
            .collect(),
    }
}

fn update_from_schema(update: &Update) -> UpdateDef {
    match update {
        Update::Assign { field, expr } => UpdateDef::Assign {
            field: ident(field),
            value: expr_from_schema(expr, false),
        },
        Update::Increment { field, amount } => UpdateDef::Increment {
            field: ident(field),
            amount: ExprDef::U64(*amount),
        },
        Update::Decrement { field, amount } => UpdateDef::Decrement {
            field: ident(field),
            amount: ExprDef::U64(*amount),
        },
        Update::MapInsert { field, key, value } => UpdateDef::MapInsert {
            field: ident(field),
            key: expr_from_schema(key, false),
            value: expr_from_schema(value, false),
        },
        Update::MapIncrement { field, key, amount } => UpdateDef::MapIncrement {
            field: ident(field),
            key: expr_from_schema(key, false),
            amount: ExprDef::U64(*amount),
        },
        Update::MapDecrement { field, key, amount } => UpdateDef::MapDecrement {
            field: ident(field),
            key: expr_from_schema(key, false),
            amount: ExprDef::U64(*amount),
        },
        Update::MapRemove { field, key } => UpdateDef::MapRemove {
            field: ident(field),
            key: expr_from_schema(key, false),
        },
        Update::SetInsert { field, value } => UpdateDef::SetInsert {
            field: ident(field),
            value: expr_from_schema(value, false),
        },
        Update::SetRemove { field, value } => UpdateDef::SetRemove {
            field: ident(field),
            value: expr_from_schema(value, false),
        },
        Update::SeqAppend { field, value } => UpdateDef::SeqAppend {
            field: ident(field),
            value: expr_from_schema(value, false),
        },
        Update::SeqPrepend { field, values } => UpdateDef::SeqPrepend {
            field: ident(field),
            values: expr_from_schema(values, false),
        },
        Update::SeqPopFront { field } => UpdateDef::SeqPopFront {
            field: ident(field),
        },
        Update::SeqRemoveValue { field, value } => UpdateDef::SeqRemoveValue {
            field: ident(field),
            value: expr_from_schema(value, false),
        },
        Update::SeqRemoveAll { field, values } => UpdateDef::SeqRemoveAll {
            field: ident(field),
            values: expr_from_schema(values, false),
        },
        Update::Conditional {
            condition,
            then_updates,
            else_updates,
        } => UpdateDef::Conditional {
            condition: expr_from_schema(condition, false),
            then_updates: then_updates.iter().map(update_from_schema).collect(),
            else_updates: else_updates.iter().map(update_from_schema).collect(),
        },
        Update::ForEach {
            binding,
            over,
            updates,
        } => UpdateDef::ForEach {
            binding: ident(binding),
            over: expr_from_schema(over, false),
            updates: updates.iter().map(update_from_schema).collect(),
        },
    }
}

fn update_from_schema_with_context(schema: &MachineSchema, update: &Update) -> UpdateDef {
    match update {
        Update::Assign { field, expr } => UpdateDef::Assign {
            field: ident(field),
            value: expr_with_target(expr, field_type(schema, field), false),
        },
        Update::Conditional {
            condition,
            then_updates,
            else_updates,
        } => UpdateDef::Conditional {
            condition: expr_from_schema(condition, false),
            then_updates: then_updates
                .iter()
                .map(|update| update_from_schema_with_context(schema, update))
                .collect(),
            else_updates: else_updates
                .iter()
                .map(|update| update_from_schema_with_context(schema, update))
                .collect(),
        },
        Update::ForEach {
            binding,
            over,
            updates,
        } => UpdateDef::ForEach {
            binding: ident(binding),
            over: expr_from_schema(over, false),
            updates: updates
                .iter()
                .map(|update| update_from_schema_with_context(schema, update))
                .collect(),
        },
        other => update_from_schema(other),
    }
}

fn expr_from_schema(schema_expr: &Expr, phase_name_as_current: bool) -> ExprDef {
    match schema_expr {
        Expr::Bool(value) => ExprDef::Bool(*value),
        Expr::U64(value) => ExprDef::U64(*value),
        Expr::String(value) => ExprDef::StringLit(value.clone()),
        Expr::NamedVariant { enum_name, variant } => ExprDef::NamedVariant {
            enum_name: ident(enum_name),
            variant: ident(variant),
        },
        Expr::EmptySet => ExprDef::EmptySet,
        Expr::EmptyMap => ExprDef::EmptyMap,
        Expr::SeqLiteral(items) => ExprDef::SeqLiteral(
            items
                .iter()
                .map(|item| expr_from_schema(item, phase_name_as_current))
                .collect(),
        ),
        Expr::CurrentPhase => ExprDef::CurrentPhase,
        Expr::Phase(phase) => ExprDef::Phase(ident(phase)),
        Expr::Field(name) if phase_name_as_current && name == "phase" => ExprDef::CurrentPhase,
        Expr::Field(name) => ExprDef::Field(ident(name)),
        Expr::Binding(name) => ExprDef::Binding(ident(name)),
        Expr::Variant(name) => ExprDef::StringLit(name.clone()),
        Expr::None => ExprDef::None,
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => ExprDef::IfElse {
            condition: Box::new(expr_from_schema(condition, phase_name_as_current)),
            then_expr: Box::new(expr_from_schema(then_expr, phase_name_as_current)),
            else_expr: Box::new(expr_from_schema(else_expr, phase_name_as_current)),
        },
        Expr::Not(inner) => ExprDef::Not(Box::new(expr_from_schema(inner, phase_name_as_current))),
        Expr::And(items) => ExprDef::And(
            items
                .iter()
                .map(|item| expr_from_schema(item, phase_name_as_current))
                .collect(),
        ),
        Expr::Or(items) => ExprDef::Or(
            items
                .iter()
                .map(|item| expr_from_schema(item, phase_name_as_current))
                .collect(),
        ),
        Expr::Eq(left, right) => ExprDef::Eq(
            Box::new(expr_from_schema_comparison_operand(
                left,
                right,
                phase_name_as_current,
            )),
            Box::new(expr_from_schema_comparison_operand(
                right,
                left,
                phase_name_as_current,
            )),
        ),
        Expr::Neq(left, right) => ExprDef::Neq(
            Box::new(expr_from_schema_comparison_operand(
                left,
                right,
                phase_name_as_current,
            )),
            Box::new(expr_from_schema_comparison_operand(
                right,
                left,
                phase_name_as_current,
            )),
        ),
        Expr::Add(left, right) => ExprDef::Add(
            Box::new(expr_from_schema(left, phase_name_as_current)),
            Box::new(expr_from_schema(right, phase_name_as_current)),
        ),
        Expr::Sub(left, right) => ExprDef::Sub(
            Box::new(expr_from_schema(left, phase_name_as_current)),
            Box::new(expr_from_schema(right, phase_name_as_current)),
        ),
        Expr::Gt(left, right) => ExprDef::Gt(
            Box::new(expr_from_schema(left, phase_name_as_current)),
            Box::new(expr_from_schema(right, phase_name_as_current)),
        ),
        Expr::Gte(left, right) => ExprDef::Gte(
            Box::new(expr_from_schema(left, phase_name_as_current)),
            Box::new(expr_from_schema(right, phase_name_as_current)),
        ),
        Expr::Lt(left, right) => ExprDef::Lt(
            Box::new(expr_from_schema(left, phase_name_as_current)),
            Box::new(expr_from_schema(right, phase_name_as_current)),
        ),
        Expr::Lte(left, right) => ExprDef::Lte(
            Box::new(expr_from_schema(left, phase_name_as_current)),
            Box::new(expr_from_schema(right, phase_name_as_current)),
        ),
        Expr::Contains { collection, value } => ExprDef::Contains {
            collection: Box::new(expr_from_schema(collection, phase_name_as_current)),
            value: Box::new(expr_from_schema(value, phase_name_as_current)),
        },
        Expr::MapContainsKey { map, key } => ExprDef::MapContainsKey {
            map: Box::new(expr_from_schema(map, phase_name_as_current)),
            key: Box::new(expr_from_schema(key, phase_name_as_current)),
        },
        Expr::SeqStartsWith { seq, prefix } => ExprDef::SeqStartsWith {
            seq: Box::new(expr_from_schema(seq, phase_name_as_current)),
            prefix: Box::new(expr_from_schema(prefix, phase_name_as_current)),
        },
        Expr::SeqElements(inner) => {
            ExprDef::SeqElements(Box::new(expr_from_schema(inner, phase_name_as_current)))
        }
        Expr::Len(inner) => ExprDef::Len(Box::new(expr_from_schema(inner, phase_name_as_current))),
        Expr::Head(inner) => {
            ExprDef::Head(Box::new(expr_from_schema(inner, phase_name_as_current)))
        }
        Expr::MapKeys(inner) => {
            ExprDef::MapKeys(Box::new(expr_from_schema(inner, phase_name_as_current)))
        }
        Expr::MapGet { map, key } => ExprDef::MapGetCloned {
            map: Box::new(expr_from_schema(map, phase_name_as_current)),
            key: Box::new(expr_from_schema(key, phase_name_as_current)),
        },
        Expr::Some(inner) => {
            ExprDef::Some(Box::new(expr_from_schema(inner, phase_name_as_current)))
        }
        Expr::Call { helper, args } => ExprDef::Call {
            helper: ident(helper),
            args: args
                .iter()
                .map(|arg| expr_from_schema(arg, phase_name_as_current))
                .collect(),
        },
        Expr::Quantified {
            quantifier,
            binding,
            over,
            body,
        } => match quantifier {
            Quantifier::All => ExprDef::ForAll {
                binding: ident(binding),
                over: Box::new(expr_from_schema(over, phase_name_as_current)),
                body: Box::new(expr_from_schema(body, phase_name_as_current)),
            },
            Quantifier::Any => ExprDef::Exists {
                binding: ident(binding),
                over: Box::new(expr_from_schema(over, phase_name_as_current)),
                body: Box::new(expr_from_schema(body, phase_name_as_current)),
            },
        },
    }
}

fn expr_from_schema_comparison_operand(
    schema_expr: &Expr,
    peer_expr: &Expr,
    phase_name_as_current: bool,
) -> ExprDef {
    match schema_expr {
        Expr::MapGet { map, key }
            if matches!(peer_expr, Expr::None | Expr::Some(_))
                && schema_map_get_flattens_option(map) =>
        {
            ExprDef::MapGetFlatten {
                map: Box::new(expr_from_schema(map, phase_name_as_current)),
                key: Box::new(expr_from_schema(key, phase_name_as_current)),
            }
        }
        Expr::MapGet { map, key } => ExprDef::MapGet {
            map: Box::new(expr_from_schema(map, phase_name_as_current)),
            key: Box::new(expr_from_schema(key, phase_name_as_current)),
        },
        _ => expr_from_schema(schema_expr, phase_name_as_current),
    }
}

fn schema_map_get_flattens_option(map_expr: &Expr) -> bool {
    matches!(
        map_expr,
        Expr::Field(name)
            if matches!(
                name.as_str(),
                "step_status" | "step_condition_results" | "step_branches" | "node_branches"
            )
    )
}

fn expr_with_target(
    schema_expr: &Expr,
    target_type: Option<&TypeRef>,
    phase_name_as_current: bool,
) -> ExprDef {
    match (target_type, schema_expr) {
        (Some(TypeRef::Named(type_name)), Expr::String(value)) => ExprDef::ConstructNamed {
            type_name: ident(type_name),
            value: value.clone(),
        },
        _ => expr_from_schema(schema_expr, phase_name_as_current),
    }
}

fn field_type<'a>(schema: &'a MachineSchema, field_name: &str) -> Option<&'a TypeRef> {
    schema
        .state
        .fields
        .iter()
        .find(|field| field.name == field_name)
        .map(|field| &field.ty)
}

fn disposition(rule: &EffectDispositionRule) -> DispositionDef {
    DispositionDef {
        effect: ident(&rule.effect_variant),
        kind: match &rule.disposition {
            EffectDisposition::Local => DispositionKind::Local,
            EffectDisposition::External => DispositionKind::External,
            EffectDisposition::Routed { consumer_machines } => {
                DispositionKind::Routed(consumer_machines.iter().map(|name| ident(name)).collect())
            }
        },
    }
}

fn ident(name: &str) -> Ident {
    Ident::new(name, Span::call_site())
}
