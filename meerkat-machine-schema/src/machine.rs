use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineSchema {
    pub machine: String,
    pub version: u32,
    pub rust: RustBinding,
    pub state: StateSchema,
    pub inputs: EnumSchema,
    pub effects: EnumSchema,
    pub helpers: Vec<HelperSchema>,
    pub derived: Vec<HelperSchema>,
    pub invariants: Vec<InvariantSchema>,
    pub transitions: Vec<TransitionSchema>,
}

impl MachineSchema {
    pub fn validate(&self) -> Result<(), MachineSchemaError> {
        let phase_names = self.state.phase.variants_by_name()?;
        let input_variants = self.inputs.variants_by_name()?;
        let effect_variants = self.effects.variants_by_name()?;
        let field_names = self.state.fields_by_name()?;
        let helper_names = unique_names(
            self.helpers
                .iter()
                .map(|helper| helper.name.as_str())
                .chain(self.derived.iter().map(|helper| helper.name.as_str())),
            "helper/derived",
        )?;

        if !phase_names.contains(&self.state.init.phase) {
            return Err(MachineSchemaError::UnknownPhase {
                phase: self.state.init.phase.clone(),
            });
        }

        for terminal in &self.state.terminal_phases {
            if !phase_names.contains(terminal) {
                return Err(MachineSchemaError::UnknownPhase {
                    phase: terminal.clone(),
                });
            }
        }

        for initializer in &self.state.init.fields {
            if !field_names.contains(initializer.field.as_str()) {
                return Err(MachineSchemaError::UnknownField {
                    field: initializer.field.clone(),
                });
            }
        }

        for invariant in &self.invariants {
            if invariant.name.is_empty() {
                return Err(MachineSchemaError::EmptyName("invariant"));
            }
            invariant.expr.validate(
                &phase_names,
                &field_names,
                &input_variants,
                &effect_variants,
                &helper_names,
                &IndexSet::new(),
            )?;
        }

        let mut transition_names = IndexSet::new();
        for transition in &self.transitions {
            if transition.name.is_empty() {
                return Err(MachineSchemaError::EmptyName("transition"));
            }
            if !transition_names.insert(transition.name.as_str()) {
                return Err(MachineSchemaError::DuplicateName {
                    kind: "transition",
                    name: transition.name.clone(),
                });
            }
            for from in &transition.from {
                if !phase_names.contains(from) {
                    return Err(MachineSchemaError::UnknownPhase {
                        phase: from.clone(),
                    });
                }
            }
            if !input_variants.contains(&transition.on.variant) {
                return Err(MachineSchemaError::UnknownInputVariant {
                    variant: transition.on.variant.clone(),
                });
            }
            if !phase_names.contains(&transition.to) {
                return Err(MachineSchemaError::UnknownPhase {
                    phase: transition.to.clone(),
                });
            }

            let bindings = unique_names(
                transition.on.bindings.iter().map(String::as_str),
                "transition binding",
            )?;

            for guard in &transition.guards {
                guard.expr.validate(
                    &phase_names,
                    &field_names,
                    &input_variants,
                    &effect_variants,
                    &helper_names,
                    &bindings,
                )?;
            }
            for update in &transition.updates {
                update.validate(
                    &phase_names,
                    &field_names,
                    &input_variants,
                    &effect_variants,
                    &helper_names,
                    &bindings,
                )?;
            }
            for effect in &transition.emit {
                if !effect_variants.contains(&effect.variant) {
                    return Err(MachineSchemaError::UnknownEffectVariant {
                        variant: effect.variant.clone(),
                    });
                }
                for expr in effect.fields.values() {
                    expr.validate(
                        &phase_names,
                        &field_names,
                        &input_variants,
                        &effect_variants,
                        &helper_names,
                        &bindings,
                    )?;
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustBinding {
    pub crate_name: String,
    pub module: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateSchema {
    pub phase: EnumSchema,
    pub fields: Vec<FieldSchema>,
    pub init: InitSchema,
    pub terminal_phases: Vec<String>,
}

impl StateSchema {
    fn fields_by_name(&self) -> Result<IndexSet<&str>, MachineSchemaError> {
        unique_names(
            self.fields.iter().map(|field| field.name.as_str()),
            "state field",
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitSchema {
    pub phase: String,
    pub fields: Vec<FieldInit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldInit {
    pub field: String,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumSchema {
    pub name: String,
    pub variants: Vec<VariantSchema>,
}

impl EnumSchema {
    pub(crate) fn variants_by_name(&self) -> Result<IndexSet<&String>, MachineSchemaError> {
        let mut names = IndexSet::new();
        for variant in &self.variants {
            if variant.name.is_empty() {
                return Err(MachineSchemaError::EmptyName("variant"));
            }
            if !names.insert(&variant.name) {
                return Err(MachineSchemaError::DuplicateName {
                    kind: "variant",
                    name: variant.name.clone(),
                });
            }
            unique_names(
                variant.fields.iter().map(|field| field.name.as_str()),
                "variant field",
            )?;
        }
        Ok(names)
    }

    pub fn variant_named(&self, name: &str) -> Result<&VariantSchema, MachineSchemaError> {
        self.variants
            .iter()
            .find(|variant| variant.name == name)
            .ok_or_else(|| MachineSchemaError::UnknownVariant {
                variant: name.to_owned(),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantSchema {
    pub name: String,
    pub fields: Vec<FieldSchema>,
}

impl VariantSchema {
    pub fn field_named(&self, name: &str) -> Result<&FieldSchema, MachineSchemaError> {
        self.fields
            .iter()
            .find(|field| field.name == name)
            .ok_or_else(|| MachineSchemaError::UnknownVariantField {
                variant: self.name.clone(),
                field: name.to_owned(),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldSchema {
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeRef {
    Bool,
    U32,
    U64,
    String,
    Named(String),
    Option(Box<TypeRef>),
    Set(Box<TypeRef>),
    Seq(Box<TypeRef>),
    Map(Box<TypeRef>, Box<TypeRef>),
}

pub type FieldType = TypeRef;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperSchema {
    pub name: String,
    pub params: Vec<FieldSchema>,
    pub returns: TypeRef,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvariantSchema {
    pub name: String,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransitionSchema {
    pub name: String,
    pub from: Vec<String>,
    pub on: InputMatch,
    pub guards: Vec<Guard>,
    pub updates: Vec<Update>,
    pub to: String,
    pub emit: Vec<EffectEmit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputMatch {
    pub variant: String,
    pub bindings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Guard {
    pub name: String,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectEmit {
    pub variant: String,
    pub fields: IndexMap<String, Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Update {
    Assign {
        field: String,
        expr: Expr,
    },
    Increment {
        field: String,
        amount: u64,
    },
    Decrement {
        field: String,
        amount: u64,
    },
    MapInsert {
        field: String,
        key: Expr,
        value: Expr,
    },
    SetInsert {
        field: String,
        value: Expr,
    },
    SetRemove {
        field: String,
        value: Expr,
    },
    SeqAppend {
        field: String,
        value: Expr,
    },
    SeqPrepend {
        field: String,
        values: Expr,
    },
    SeqPopFront {
        field: String,
    },
    SeqRemoveValue {
        field: String,
        value: Expr,
    },
    SeqRemoveAll {
        field: String,
        values: Expr,
    },
    Conditional {
        condition: Expr,
        then_updates: Vec<Update>,
        else_updates: Vec<Update>,
    },
    ForEach {
        binding: String,
        over: Expr,
        updates: Vec<Update>,
    },
}

impl Update {
    fn validate(
        &self,
        phase_names: &IndexSet<&String>,
        field_names: &IndexSet<&str>,
        input_variants: &IndexSet<&String>,
        effect_variants: &IndexSet<&String>,
        helper_names: &IndexSet<&str>,
        bindings: &IndexSet<&str>,
    ) -> Result<(), MachineSchemaError> {
        match self {
            Self::Assign { field, .. }
            | Self::Increment { field, .. }
            | Self::Decrement { field, .. }
            | Self::SeqPopFront { field } => {
                if !field_names.contains(field.as_str()) {
                    return Err(MachineSchemaError::UnknownField {
                        field: field.clone(),
                    });
                }
                if let Self::Assign { expr, .. } = self {
                    expr.validate(
                        phase_names,
                        field_names,
                        input_variants,
                        effect_variants,
                        helper_names,
                        bindings,
                    )?;
                }
            }
            Self::MapInsert { field, key, value } => {
                if !field_names.contains(field.as_str()) {
                    return Err(MachineSchemaError::UnknownField {
                        field: field.clone(),
                    });
                }
                key.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                value.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::SetInsert { field, value }
            | Self::SetRemove { field, value }
            | Self::SeqAppend { field, value }
            | Self::SeqRemoveValue { field, value } => {
                if !field_names.contains(field.as_str()) {
                    return Err(MachineSchemaError::UnknownField {
                        field: field.clone(),
                    });
                }
                value.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::SeqPrepend { field, values } | Self::SeqRemoveAll { field, values } => {
                if !field_names.contains(field.as_str()) {
                    return Err(MachineSchemaError::UnknownField {
                        field: field.clone(),
                    });
                }
                values.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::ForEach {
                binding,
                over,
                updates,
            } => {
                over.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                let mut nested_bindings = bindings.clone();
                nested_bindings.insert(binding.as_str());
                for update in updates {
                    update.validate(
                        phase_names,
                        field_names,
                        input_variants,
                        effect_variants,
                        helper_names,
                        &nested_bindings,
                    )?;
                }
            }
            Self::Conditional {
                condition,
                then_updates,
                else_updates,
            } => {
                condition.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                for update in then_updates {
                    update.validate(
                        phase_names,
                        field_names,
                        input_variants,
                        effect_variants,
                        helper_names,
                        bindings,
                    )?;
                }
                for update in else_updates {
                    update.validate(
                        phase_names,
                        field_names,
                        input_variants,
                        effect_variants,
                        helper_names,
                        bindings,
                    )?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Quantifier {
    Any,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Expr {
    Bool(bool),
    U64(u64),
    String(String),
    EmptySet,
    EmptyMap,
    SeqLiteral(Vec<Expr>),
    CurrentPhase,
    Phase(String),
    Field(String),
    Binding(String),
    Variant(String),
    None,
    IfElse {
        condition: Box<Expr>,
        then_expr: Box<Expr>,
        else_expr: Box<Expr>,
    },
    Not(Box<Expr>),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Eq(Box<Expr>, Box<Expr>),
    Neq(Box<Expr>, Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Gt(Box<Expr>, Box<Expr>),
    Gte(Box<Expr>, Box<Expr>),
    Lt(Box<Expr>, Box<Expr>),
    Lte(Box<Expr>, Box<Expr>),
    Contains {
        collection: Box<Expr>,
        value: Box<Expr>,
    },
    SeqStartsWith {
        seq: Box<Expr>,
        prefix: Box<Expr>,
    },
    Len(Box<Expr>),
    Head(Box<Expr>),
    MapKeys(Box<Expr>),
    MapGet {
        map: Box<Expr>,
        key: Box<Expr>,
    },
    Some(Box<Expr>),
    Call {
        helper: String,
        args: Vec<Expr>,
    },
    Quantified {
        quantifier: Quantifier,
        binding: String,
        over: Box<Expr>,
        body: Box<Expr>,
    },
}

impl Expr {
    fn validate(
        &self,
        phase_names: &IndexSet<&String>,
        field_names: &IndexSet<&str>,
        input_variants: &IndexSet<&String>,
        effect_variants: &IndexSet<&String>,
        helper_names: &IndexSet<&str>,
        bindings: &IndexSet<&str>,
    ) -> Result<(), MachineSchemaError> {
        match self {
            Self::Bool(_)
            | Self::U64(_)
            | Self::String(_)
            | Self::EmptySet
            | Self::EmptyMap
            | Self::None
            | Self::CurrentPhase => {}
            Self::SeqLiteral(items) => {
                for item in items {
                    item.validate(
                        phase_names,
                        field_names,
                        input_variants,
                        effect_variants,
                        helper_names,
                        bindings,
                    )?;
                }
            }
            Self::Phase(phase) => {
                if !phase_names.contains(phase) {
                    return Err(MachineSchemaError::UnknownPhase {
                        phase: phase.clone(),
                    });
                }
            }
            Self::Field(field) => {
                if !field_names.contains(field.as_str()) {
                    return Err(MachineSchemaError::UnknownField {
                        field: field.clone(),
                    });
                }
            }
            Self::Binding(binding) => {
                if !bindings.contains(binding.as_str()) {
                    return Err(MachineSchemaError::UnknownBinding {
                        binding: binding.clone(),
                    });
                }
            }
            Self::Variant(variant) => {
                if !input_variants.contains(variant) && !effect_variants.contains(variant) {
                    return Err(MachineSchemaError::UnknownVariant {
                        variant: variant.clone(),
                    });
                }
            }
            Self::IfElse {
                condition,
                then_expr,
                else_expr,
            } => {
                condition.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                then_expr.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                else_expr.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::Not(inner)
            | Self::Len(inner)
            | Self::Head(inner)
            | Self::MapKeys(inner)
            | Self::Some(inner) => inner.validate(
                phase_names,
                field_names,
                input_variants,
                effect_variants,
                helper_names,
                bindings,
            )?,
            Self::And(items) | Self::Or(items) => {
                for item in items {
                    item.validate(
                        phase_names,
                        field_names,
                        input_variants,
                        effect_variants,
                        helper_names,
                        bindings,
                    )?;
                }
            }
            Self::Eq(left, right)
            | Self::Neq(left, right)
            | Self::Add(left, right)
            | Self::Sub(left, right)
            | Self::Gt(left, right)
            | Self::Gte(left, right)
            | Self::Lt(left, right)
            | Self::Lte(left, right) => {
                left.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                right.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::Contains { collection, value } => {
                collection.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                value.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::SeqStartsWith { seq, prefix } => {
                seq.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                prefix.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::MapGet { map, key } => {
                map.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                key.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::Call { helper, args } => {
                if !helper_names.contains(helper.as_str()) {
                    return Err(MachineSchemaError::UnknownHelper {
                        helper: helper.clone(),
                    });
                }
                for arg in args {
                    arg.validate(
                        phase_names,
                        field_names,
                        input_variants,
                        effect_variants,
                        helper_names,
                        bindings,
                    )?;
                }
            }
            Self::Quantified {
                binding,
                over,
                body,
                ..
            } => {
                over.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                let mut nested_bindings = bindings.clone();
                nested_bindings.insert(binding.as_str());
                body.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    effect_variants,
                    helper_names,
                    &nested_bindings,
                )?;
            }
        }
        Ok(())
    }
}

fn unique_names<'a>(
    names: impl IntoIterator<Item = &'a str>,
    kind: &'static str,
) -> Result<IndexSet<&'a str>, MachineSchemaError> {
    let mut seen = IndexSet::new();
    for name in names {
        if name.is_empty() {
            return Err(MachineSchemaError::EmptyName(kind));
        }
        if !seen.insert(name) {
            return Err(MachineSchemaError::DuplicateName {
                kind,
                name: name.to_owned(),
            });
        }
    }
    Ok(seen)
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MachineSchemaError {
    #[error("duplicate {kind} name `{name}`")]
    DuplicateName { kind: &'static str, name: String },
    #[error("empty {0} name")]
    EmptyName(&'static str),
    #[error("unknown phase `{phase}`")]
    UnknownPhase { phase: String },
    #[error("unknown field `{field}`")]
    UnknownField { field: String },
    #[error("unknown input variant `{variant}`")]
    UnknownInputVariant { variant: String },
    #[error("unknown effect variant `{variant}`")]
    UnknownEffectVariant { variant: String },
    #[error("unknown helper `{helper}`")]
    UnknownHelper { helper: String },
    #[error("unknown binding `{binding}`")]
    UnknownBinding { binding: String },
    #[error("unknown variant `{variant}`")]
    UnknownVariant { variant: String },
    #[error("unknown field `{field}` on variant `{variant}`")]
    UnknownVariantField { variant: String, field: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{
        canonical_machine_schemas, input_lifecycle_machine, mob_lifecycle_machine,
    };

    #[test]
    fn validates_mob_orchestrator_style_machine() {
        let schema = MachineSchema {
            machine: "MobOrchestratorMachine".into(),
            version: 1,
            rust: RustBinding {
                crate_name: "meerkat-mob".into(),
                module: "machines::mob_orchestrator".into(),
            },
            state: StateSchema {
                phase: EnumSchema {
                    name: "OrchestratorState".into(),
                    variants: vec![
                        variant("Creating"),
                        variant("Running"),
                        variant("Stopped"),
                        variant("Completed"),
                        variant("Destroyed"),
                    ],
                },
                fields: vec![
                    field("coordinator_bound", TypeRef::Bool),
                    field("pending_spawn_count", TypeRef::U32),
                    field("active_flow_count", TypeRef::U32),
                    field("topology_revision", TypeRef::U32),
                    field("supervisor_active", TypeRef::Bool),
                ],
                init: InitSchema {
                    phase: "Creating".into(),
                    fields: vec![
                        init("coordinator_bound", Expr::Bool(false)),
                        init("pending_spawn_count", Expr::U64(0)),
                        init("active_flow_count", Expr::U64(0)),
                        init("topology_revision", Expr::U64(0)),
                        init("supervisor_active", Expr::Bool(false)),
                    ],
                },
                terminal_phases: vec!["Destroyed".into()],
            },
            inputs: EnumSchema {
                name: "MobOrchestratorInput".into(),
                variants: vec![
                    variant("InitializeOrchestrator"),
                    variant("BindCoordinator"),
                    variant("UnbindCoordinator"),
                    variant("StageSpawn"),
                    variant("CompleteSpawn"),
                ],
            },
            effects: EnumSchema {
                name: "MobOrchestratorEffect".into(),
                variants: vec![
                    variant("ActivateSupervisor"),
                    variant("DeactivateSupervisor"),
                    variant("EmitOrchestratorNotice"),
                ],
            },
            helpers: vec![],
            derived: vec![],
            invariants: vec![InvariantSchema {
                name: "destroyed_is_terminal".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::Field("supervisor_active".into())),
                        Box::new(Expr::Bool(true)),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Phase("Destroyed".into())),
                        Box::new(Expr::Phase("Destroyed".into())),
                    ),
                ]),
            }],
            transitions: vec![
                TransitionSchema {
                    name: "InitializeOrchestrator".into(),
                    from: vec!["Creating".into()],
                    on: InputMatch {
                        variant: "InitializeOrchestrator".into(),
                        bindings: vec![],
                    },
                    guards: vec![],
                    updates: vec![
                        Update::Assign {
                            field: "coordinator_bound".into(),
                            expr: Expr::Bool(true),
                        },
                        Update::Assign {
                            field: "supervisor_active".into(),
                            expr: Expr::Bool(true),
                        },
                    ],
                    to: "Running".into(),
                    emit: vec![EffectEmit {
                        variant: "ActivateSupervisor".into(),
                        fields: IndexMap::new(),
                    }],
                },
                TransitionSchema {
                    name: "StageSpawn".into(),
                    from: vec!["Running".into()],
                    on: InputMatch {
                        variant: "StageSpawn".into(),
                        bindings: vec![],
                    },
                    guards: vec![Guard {
                        name: "coordinator_is_bound".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("coordinator_bound".into())),
                            Box::new(Expr::Bool(true)),
                        ),
                    }],
                    updates: vec![Update::Increment {
                        field: "pending_spawn_count".into(),
                        amount: 1,
                    }],
                    to: "Running".into(),
                    emit: vec![],
                },
            ],
        };

        assert_eq!(schema.validate(), Ok(()));
    }

    #[test]
    fn validates_peer_comms_style_machine() {
        let schema = MachineSchema {
            machine: "PeerCommsMachine".into(),
            version: 1,
            rust: RustBinding {
                crate_name: "meerkat-comms".into(),
                module: "machines::peer_comms".into(),
            },
            state: StateSchema {
                phase: EnumSchema {
                    name: "PeerIngressState".into(),
                    variants: vec![
                        variant("Absent"),
                        variant("Received"),
                        variant("Dropped"),
                        variant("Submitted"),
                        variant("Delivered"),
                    ],
                },
                fields: vec![
                    field(
                        "trusted_peers",
                        TypeRef::Set(Box::new(TypeRef::Named("PeerId".into()))),
                    ),
                    field(
                        "raw_item_peer",
                        TypeRef::Map(
                            Box::new(TypeRef::Named("RawItemId".into())),
                            Box::new(TypeRef::Option(Box::new(TypeRef::Named("PeerId".into())))),
                        ),
                    ),
                    field(
                        "raw_item_kind",
                        TypeRef::Map(
                            Box::new(TypeRef::Named("RawItemId".into())),
                            Box::new(TypeRef::Named("RawPeerKind".into())),
                        ),
                    ),
                    field(
                        "classified_as",
                        TypeRef::Map(
                            Box::new(TypeRef::Named("RawItemId".into())),
                            Box::new(TypeRef::Option(Box::new(TypeRef::Named(
                                "PeerInputClass".into(),
                            )))),
                        ),
                    ),
                    field(
                        "trusted_snapshot",
                        TypeRef::Map(
                            Box::new(TypeRef::Named("RawItemId".into())),
                            Box::new(TypeRef::Bool),
                        ),
                    ),
                    field(
                        "submission_queue",
                        TypeRef::Seq(Box::new(TypeRef::Named("RawItemId".into()))),
                    ),
                ],
                init: InitSchema {
                    phase: "Absent".into(),
                    fields: vec![],
                },
                terminal_phases: vec!["Dropped".into(), "Delivered".into()],
            },
            inputs: EnumSchema {
                name: "PeerCommsInput".into(),
                variants: vec![
                    VariantSchema {
                        name: "TrustPeer".into(),
                        fields: vec![field("peer_id", TypeRef::Named("PeerId".into()))],
                    },
                    VariantSchema {
                        name: "ReceivePeerEnvelope".into(),
                        fields: vec![
                            field("raw_item_id", TypeRef::Named("RawItemId".into())),
                            field("peer_id", TypeRef::Named("PeerId".into())),
                            field("raw_kind", TypeRef::Named("RawPeerKind".into())),
                        ],
                    },
                    VariantSchema {
                        name: "SubmitTypedPeerInput".into(),
                        fields: vec![field("raw_item_id", TypeRef::Named("RawItemId".into()))],
                    },
                ],
            },
            effects: EnumSchema {
                name: "PeerCommsEffect".into(),
                variants: vec![VariantSchema {
                    name: "SubmitPeerInputCandidate".into(),
                    fields: vec![
                        field("raw_item_id", TypeRef::Named("RawItemId".into())),
                        field("peer_input_class", TypeRef::Named("PeerInputClass".into())),
                    ],
                }],
            },
            helpers: vec![HelperSchema {
                name: "ClassFor".into(),
                params: vec![field("raw_kind", TypeRef::Named("RawPeerKind".into()))],
                returns: TypeRef::Named("PeerInputClass".into()),
                body: Expr::Binding("raw_kind".into()),
            }],
            derived: vec![],
            invariants: vec![InvariantSchema {
                name: "delivered_items_are_not_queued".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "raw_item_id".into(),
                    over: Box::new(Expr::Field("submission_queue".into())),
                    body: Box::new(Expr::Neq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("classified_as".into())),
                            key: Box::new(Expr::Binding("raw_item_id".into())),
                        }),
                        Box::new(Expr::None),
                    )),
                },
            }],
            transitions: vec![
                TransitionSchema {
                    name: "TrustPeer".into(),
                    from: vec!["Absent".into(), "Received".into(), "Submitted".into()],
                    on: InputMatch {
                        variant: "TrustPeer".into(),
                        bindings: vec!["peer_id".into()],
                    },
                    guards: vec![],
                    updates: vec![Update::SetInsert {
                        field: "trusted_peers".into(),
                        value: Expr::Binding("peer_id".into()),
                    }],
                    to: "Absent".into(),
                    emit: vec![],
                },
                TransitionSchema {
                    name: "SubmitTypedPeerInput".into(),
                    from: vec!["Received".into()],
                    on: InputMatch {
                        variant: "SubmitTypedPeerInput".into(),
                        bindings: vec!["raw_item_id".into()],
                    },
                    guards: vec![Guard {
                        name: "item_was_classified".into(),
                        expr: Expr::Contains {
                            collection: Box::new(Expr::Field("submission_queue".into())),
                            value: Box::new(Expr::Binding("raw_item_id".into())),
                        },
                    }],
                    updates: vec![Update::MapInsert {
                        field: "trusted_snapshot".into(),
                        key: Expr::Binding("raw_item_id".into()),
                        value: Expr::Bool(true),
                    }],
                    to: "Submitted".into(),
                    emit: vec![EffectEmit {
                        variant: "SubmitPeerInputCandidate".into(),
                        fields: IndexMap::from([
                            ("raw_item_id".into(), Expr::Binding("raw_item_id".into())),
                            (
                                "peer_input_class".into(),
                                Expr::Call {
                                    helper: "ClassFor".into(),
                                    args: vec![Expr::Variant("ReceivePeerEnvelope".into())],
                                },
                            ),
                        ]),
                    }],
                },
            ],
        };

        assert_eq!(schema.validate(), Ok(()));
    }

    #[test]
    fn validates_input_lifecycle_machine_definition() {
        let schema = input_lifecycle_machine();
        assert_eq!(schema.validate(), Ok(()));
    }

    #[test]
    fn validates_mob_lifecycle_machine_definition() {
        let schema = mob_lifecycle_machine();
        assert_eq!(schema.validate(), Ok(()));
    }

    #[test]
    fn canonical_machine_registry_is_individually_valid() {
        for schema in canonical_machine_schemas() {
            assert_eq!(
                schema.validate(),
                Ok(()),
                "machine {} should validate",
                schema.machine
            );
        }
    }

    fn variant(name: &str) -> VariantSchema {
        VariantSchema {
            name: name.into(),
            fields: vec![],
        }
    }

    fn field(name: &str, ty: TypeRef) -> FieldSchema {
        FieldSchema {
            name: name.into(),
            ty,
        }
    }

    fn init(field: &str, expr: Expr) -> FieldInit {
        FieldInit {
            field: field.into(),
            expr,
        }
    }
}
