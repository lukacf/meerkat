use indexmap::{IndexMap, IndexSet};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineSchema {
    pub machine: String,
    pub version: u32,
    pub rust: RustBinding,
    pub state: StateSchema,
    pub inputs: EnumSchema,
    pub surface_only_inputs: Vec<String>,
    pub signals: EnumSchema,
    pub effects: EnumSchema,
    pub helpers: Vec<HelperSchema>,
    pub derived: Vec<HelperSchema>,
    pub invariants: Vec<InvariantSchema>,
    pub transitions: Vec<TransitionSchema>,
    pub effect_dispositions: Vec<EffectDispositionRule>,
    /// Override the CI step_limit for individual machine TLC verification.
    /// Machines with many state fields (e.g. a rich MobMachine flow/work region)
    /// may need a lower limit to keep CI verification tractable. `None` uses the
    /// codegen default (6 for CI, 8 for deep).
    pub ci_step_limit: Option<u32>,
}

pub fn authoritative_named_enum_variants(machine_name: &str) -> BTreeMap<String, BTreeSet<String>> {
    match machine_name {
        "FlowFrameMachine" => BTreeMap::from([
            ("DependencyMode".into(), named_enum_variants(["All", "Any"])),
            ("FlowNodeKind".into(), named_enum_variants(["Loop", "Step"])),
            ("FrameScope".into(), named_enum_variants(["Body", "Root"])),
            (
                "NodeRunStatus".into(),
                named_enum_variants([
                    "Pending",
                    "Ready",
                    "Running",
                    "Completed",
                    "Failed",
                    "Skipped",
                    "Canceled",
                ]),
            ),
        ]),
        "FlowRunMachine" => BTreeMap::from([
            (
                "CollectionPolicyKind".into(),
                named_enum_variants(["All", "Any", "Quorum"]),
            ),
            ("DependencyMode".into(), named_enum_variants(["All", "Any"])),
            (
                "FlowRunStatus".into(),
                named_enum_variants(["Pending", "Running", "Completed", "Failed", "Canceled"]),
            ),
            (
                "StepRunStatus".into(),
                named_enum_variants(["Dispatched", "Completed", "Failed", "Skipped", "Canceled"]),
            ),
        ]),
        "LoopIterationMachine" => BTreeMap::from([(
            "LoopIterationStage".into(),
            named_enum_variants(["AwaitingBodyFrame", "BodyFrameActive", "AwaitingUntil"]),
        )]),
        _ => BTreeMap::new(),
    }
}

fn named_enum_variants(values: impl IntoIterator<Item = &'static str>) -> BTreeSet<String> {
    values.into_iter().map(str::to_owned).collect()
}

impl MachineSchema {
    pub fn validate(&self) -> Result<(), MachineSchemaError> {
        let phase_names = self.state.phase.variants_by_name()?;
        let input_variants = self.inputs.variants_by_name()?;
        let surface_only_inputs = unique_names(
            self.surface_only_inputs.iter().map(String::as_str),
            "surface-only input",
        )?;
        let signal_variants = self.signals.variants_by_name()?;
        let effect_variants = self.effects.variants_by_name()?;
        let field_names = self.state.fields_by_name()?;
        let helper_names = unique_names(
            self.helpers
                .iter()
                .map(|helper| helper.name.as_str())
                .chain(self.derived.iter().map(|helper| helper.name.as_str())),
            "helper/derived",
        )?;
        validate_generated_accessor_names(
            "phase",
            self.state
                .phase
                .variants
                .iter()
                .map(|variant| variant.name.as_str()),
        )?;
        validate_generated_accessor_names(
            "field",
            self.state
                .fields
                .iter()
                .map(|field| field.name.as_str())
                .chain(
                    self.inputs
                        .variants
                        .iter()
                        .flat_map(|variant| variant.fields.iter().map(|field| field.name.as_str())),
                )
                .chain(
                    self.signals
                        .variants
                        .iter()
                        .flat_map(|variant| variant.fields.iter().map(|field| field.name.as_str())),
                )
                .chain(
                    self.effects
                        .variants
                        .iter()
                        .flat_map(|variant| variant.fields.iter().map(|field| field.name.as_str())),
                )
                .chain(
                    self.helpers
                        .iter()
                        .flat_map(|helper| helper.params.iter().map(|field| field.name.as_str())),
                )
                .chain(
                    self.derived
                        .iter()
                        .flat_map(|helper| helper.params.iter().map(|field| field.name.as_str())),
                ),
        )?;
        validate_generated_accessor_names(
            "input",
            self.inputs
                .variants
                .iter()
                .map(|variant| variant.name.as_str()),
        )?;
        validate_generated_accessor_names(
            "signal",
            self.signals
                .variants
                .iter()
                .map(|variant| variant.name.as_str()),
        )?;
        validate_generated_accessor_names(
            "effect",
            self.effects
                .variants
                .iter()
                .map(|variant| variant.name.as_str()),
        )?;
        validate_generated_accessor_names(
            "helper",
            self.helpers
                .iter()
                .map(|helper| helper.name.as_str())
                .chain(self.derived.iter().map(|helper| helper.name.as_str())),
        )?;
        validate_generated_accessor_names(
            "transition",
            self.transitions
                .iter()
                .map(|transition| transition.name.as_str()),
        )?;
        let named_variants = collect_named_variant_accessors(self);
        let authoritative_named_variants = authoritative_named_enum_variants(&self.machine);
        if !authoritative_named_variants.is_empty() {
            for (enum_name, variants) in &named_variants {
                let Some(allowed) = authoritative_named_variants.get(enum_name) else {
                    return Err(MachineSchemaError::UnknownNamedVariantEnum {
                        enum_name: enum_name.clone(),
                    });
                };
                for variant in variants {
                    if !allowed.contains(variant) {
                        return Err(MachineSchemaError::UnknownNamedVariantValue {
                            enum_name: enum_name.clone(),
                            variant: variant.clone(),
                        });
                    }
                }
            }
            validate_generated_accessor_names_allowing_keywords(
                "named variant enum",
                authoritative_named_variants.keys().map(String::as_str),
            )?;
            for variants in authoritative_named_variants.values() {
                validate_generated_accessor_names_allowing_keywords(
                    "named variant value",
                    variants.iter().map(String::as_str),
                )?;
            }
        }

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

        for surface_only_input in &self.surface_only_inputs {
            if !input_variants
                .iter()
                .any(|variant| variant.as_str() == surface_only_input)
            {
                return Err(MachineSchemaError::UnknownSurfaceOnlyInputVariant {
                    variant: surface_only_input.clone(),
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
                &signal_variants,
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
            match transition.on.kind {
                TriggerKind::Input if !input_variants.contains(&transition.on.variant) => {
                    return Err(MachineSchemaError::UnknownInputVariant {
                        variant: transition.on.variant.clone(),
                    });
                }
                TriggerKind::Signal if !signal_variants.contains(&transition.on.variant) => {
                    return Err(MachineSchemaError::UnknownSignalVariant {
                        variant: transition.on.variant.clone(),
                    });
                }
                _ => {}
            }
            if transition.on.kind == TriggerKind::Input
                && surface_only_inputs.contains(transition.on.variant.as_str())
            {
                return Err(MachineSchemaError::SurfaceOnlyInputHasTransition {
                    variant: transition.on.variant.clone(),
                    transition: transition.name.clone(),
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
            let trigger_variant = match transition.on.kind {
                TriggerKind::Input => match self.inputs.variant_named(&transition.on.variant) {
                    Ok(variant) => variant,
                    Err(_) => {
                        return Err(MachineSchemaError::UnknownInputVariant {
                            variant: transition.on.variant.clone(),
                        });
                    }
                },
                TriggerKind::Signal => match self.signals.variant_named(&transition.on.variant) {
                    Ok(variant) => variant,
                    Err(_) => {
                        return Err(MachineSchemaError::UnknownSignalVariant {
                            variant: transition.on.variant.clone(),
                        });
                    }
                },
            };
            for binding in &bindings {
                if trigger_variant.field_named(binding).is_err() {
                    return Err(MachineSchemaError::UnknownVariantField {
                        variant: transition.on.variant.clone(),
                        field: (*binding).to_owned(),
                    });
                }
            }

            for guard in &transition.guards {
                guard.expr.validate(
                    &phase_names,
                    &field_names,
                    &input_variants,
                    &signal_variants,
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
                    &signal_variants,
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
                        &signal_variants,
                        &effect_variants,
                        &helper_names,
                        &bindings,
                    )?;
                }
            }
        }

        // Validate effect dispositions: every rule must reference a known effect variant,
        // no duplicates, and when dispositions are present every effect must be covered.
        if !self.effect_dispositions.is_empty() {
            let mut disposed_variants = IndexSet::new();
            for rule in &self.effect_dispositions {
                if !effect_variants.contains(&rule.effect_variant) {
                    return Err(MachineSchemaError::UnknownEffectDispositionVariant {
                        variant: rule.effect_variant.clone(),
                    });
                }
                if !disposed_variants.insert(&rule.effect_variant) {
                    return Err(MachineSchemaError::DuplicateEffectDisposition {
                        variant: rule.effect_variant.clone(),
                    });
                }
                if rule.handoff_protocol.is_some()
                    && matches!(rule.disposition, EffectDisposition::Routed { .. })
                {
                    return Err(MachineSchemaError::HandoffProtocolOnRoutedEffect {
                        variant: rule.effect_variant.clone(),
                    });
                }
            }
            for variant in &effect_variants {
                if !disposed_variants.contains(*variant) {
                    return Err(MachineSchemaError::MissingEffectDisposition {
                        variant: (*variant).clone(),
                    });
                }
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectDisposition {
    /// Handled internally by the owning shell/runtime. No cross-machine routing needed.
    Local,
    /// Observability/external side effect. No machine consumption expected.
    External,
    /// Must be routed to a consumer machine when both producer and consumer coexist in a composition.
    Routed { consumer_machines: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectDispositionRule {
    pub effect_variant: String,
    pub disposition: EffectDisposition,
    /// When set, this effect participates in an owner-handoff protocol.
    /// The named protocol must be declared as an `EffectHandoffProtocol`
    /// in every composition that includes this machine.
    /// Only meaningful for `Local` or `External` dispositions.
    pub handoff_protocol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustBinding {
    pub crate_name: String,
    pub module: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitSchema {
    pub phase: String,
    pub fields: Vec<FieldInit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInit {
    pub field: String,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSchema {
    pub name: String,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    Bool,
    U32,
    U64,
    String,
    Named(String),
    Enum(String),
    Option(Box<TypeRef>),
    Set(Box<TypeRef>),
    Seq(Box<TypeRef>),
    Map(Box<TypeRef>, Box<TypeRef>),
}

pub type FieldType = TypeRef;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperSchema {
    pub name: String,
    pub params: Vec<FieldSchema>,
    pub returns: TypeRef,
    pub body: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantSchema {
    pub name: String,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionSchema {
    pub name: String,
    pub from: Vec<String>,
    pub on: TriggerMatch,
    pub guards: Vec<Guard>,
    pub updates: Vec<Update>,
    pub to: String,
    pub emit: Vec<EffectEmit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    Input,
    Signal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerMatch {
    pub kind: TriggerKind,
    pub variant: String,
    pub bindings: Vec<String>,
}

pub type InputMatch = TriggerMatch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guard {
    pub name: String,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectEmit {
    pub variant: String,
    pub fields: IndexMap<String, Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    MapIncrement {
        field: String,
        key: Expr,
        amount: u64,
    },
    MapDecrement {
        field: String,
        key: Expr,
        amount: u64,
    },
    MapRemove {
        field: String,
        key: Expr,
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
    #[allow(clippy::too_many_arguments)]
    fn validate(
        &self,
        phase_names: &IndexSet<&String>,
        field_names: &IndexSet<&str>,
        input_variants: &IndexSet<&String>,
        signal_variants: &IndexSet<&String>,
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
                        signal_variants,
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
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                value.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::MapRemove { field, key } => {
                if !field_names.contains(field.as_str()) {
                    return Err(MachineSchemaError::UnknownField {
                        field: field.clone(),
                    });
                }
                key.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::MapIncrement { field, key, .. } | Self::MapDecrement { field, key, .. } => {
                if !field_names.contains(field.as_str()) {
                    return Err(MachineSchemaError::UnknownField {
                        field: field.clone(),
                    });
                }
                key.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
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
                    signal_variants,
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
                    signal_variants,
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
                    signal_variants,
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
                        signal_variants,
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
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                for update in then_updates {
                    update.validate(
                        phase_names,
                        field_names,
                        input_variants,
                        signal_variants,
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
                        signal_variants,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quantifier {
    Any,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expr {
    Bool(bool),
    U64(u64),
    String(String),
    NamedVariant {
        enum_name: String,
        variant: String,
    },
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
    MapContainsKey {
        map: Box<Expr>,
        key: Box<Expr>,
    },
    SeqStartsWith {
        seq: Box<Expr>,
        prefix: Box<Expr>,
    },
    SeqElements(Box<Expr>),
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
    #[allow(clippy::too_many_arguments)]
    fn validate(
        &self,
        phase_names: &IndexSet<&String>,
        field_names: &IndexSet<&str>,
        input_variants: &IndexSet<&String>,
        signal_variants: &IndexSet<&String>,
        effect_variants: &IndexSet<&String>,
        helper_names: &IndexSet<&str>,
        bindings: &IndexSet<&str>,
    ) -> Result<(), MachineSchemaError> {
        match self {
            Self::Bool(_)
            | Self::U64(_)
            | Self::String(_)
            | Self::NamedVariant { .. }
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
                        signal_variants,
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
                if !input_variants.contains(variant)
                    && !signal_variants.contains(variant)
                    && !effect_variants.contains(variant)
                {
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
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                then_expr.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                else_expr.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
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
                signal_variants,
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
                        signal_variants,
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
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                right.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
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
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                value.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::MapContainsKey { map, key } => {
                map.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                key.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
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
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                prefix.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
            }
            Self::SeqElements(inner) => {
                inner.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
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
                    signal_variants,
                    effect_variants,
                    helper_names,
                    bindings,
                )?;
                key.validate(
                    phase_names,
                    field_names,
                    input_variants,
                    signal_variants,
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
                        signal_variants,
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
                    signal_variants,
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
                    signal_variants,
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

fn validate_generated_accessor_names<'a>(
    namespace: &'static str,
    names: impl IntoIterator<Item = &'a str>,
) -> Result<(), MachineSchemaError> {
    validate_generated_accessor_names_with_keywords(namespace, names, false)
}

fn validate_generated_accessor_names_allowing_keywords<'a>(
    namespace: &'static str,
    names: impl IntoIterator<Item = &'a str>,
) -> Result<(), MachineSchemaError> {
    validate_generated_accessor_names_with_keywords(namespace, names, true)
}

fn validate_generated_accessor_names_with_keywords<'a>(
    namespace: &'static str,
    names: impl IntoIterator<Item = &'a str>,
    allow_keywords: bool,
) -> Result<(), MachineSchemaError> {
    let mut raw_seen = IndexSet::new();
    let mut accessor_to_raw = IndexMap::<String, String>::new();

    for raw_name in names {
        if !raw_seen.insert(raw_name) {
            continue;
        }

        let accessor = generated_accessor_name(raw_name);
        if accessor.is_empty() {
            return Err(MachineSchemaError::InvalidGeneratedAccessorName {
                namespace,
                name: raw_name.to_owned(),
                accessor,
                reason: "normalizes to an empty Rust identifier",
            });
        }
        if accessor.starts_with(|ch: char| ch.is_ascii_digit()) {
            return Err(MachineSchemaError::InvalidGeneratedAccessorName {
                namespace,
                name: raw_name.to_owned(),
                accessor,
                reason: "normalizes to an identifier that starts with a digit",
            });
        }
        if !accessor
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
        {
            return Err(MachineSchemaError::InvalidGeneratedAccessorName {
                namespace,
                name: raw_name.to_owned(),
                accessor,
                reason: "normalizes to an identifier with unsupported characters",
            });
        }
        if !allow_keywords && rust_keyword(&accessor) {
            return Err(MachineSchemaError::InvalidGeneratedAccessorName {
                namespace,
                name: raw_name.to_owned(),
                accessor,
                reason: "normalizes to a reserved Rust keyword",
            });
        }

        if let Some(previous_name) = accessor_to_raw.insert(accessor.clone(), raw_name.to_owned())
            && previous_name != raw_name
        {
            return Err(MachineSchemaError::GeneratedAccessorCollision {
                namespace,
                accessor,
                first_name: previous_name,
                second_name: raw_name.to_owned(),
            });
        }
    }

    Ok(())
}

fn collect_named_variant_accessors(schema: &MachineSchema) -> BTreeMap<String, BTreeSet<String>> {
    let mut variants = BTreeMap::new();

    for init in &schema.state.init.fields {
        collect_named_variants_from_expr(&init.expr, &mut variants);
    }

    for helper in schema.helpers.iter().chain(schema.derived.iter()) {
        collect_named_variants_from_expr(&helper.body, &mut variants);
    }

    for invariant in &schema.invariants {
        collect_named_variants_from_expr(&invariant.expr, &mut variants);
    }

    for transition in &schema.transitions {
        for guard in &transition.guards {
            collect_named_variants_from_expr(&guard.expr, &mut variants);
        }
        for update in &transition.updates {
            collect_named_variants_from_update(update, &mut variants);
        }
        for effect in &transition.emit {
            for expr in effect.fields.values() {
                collect_named_variants_from_expr(expr, &mut variants);
            }
        }
    }

    variants
}

fn collect_named_variants_from_update(
    update: &Update,
    variants: &mut BTreeMap<String, BTreeSet<String>>,
) {
    match update {
        Update::Assign { expr, .. } => collect_named_variants_from_expr(expr, variants),
        Update::MapInsert { key, value, .. } => {
            collect_named_variants_from_expr(key, variants);
            collect_named_variants_from_expr(value, variants);
        }
        Update::MapIncrement { key, .. }
        | Update::MapDecrement { key, .. }
        | Update::MapRemove { key, .. } => collect_named_variants_from_expr(key, variants),
        Update::SetInsert { value, .. }
        | Update::SetRemove { value, .. }
        | Update::SeqAppend { value, .. }
        | Update::SeqRemoveValue { value, .. } => collect_named_variants_from_expr(value, variants),
        Update::SeqPrepend { values, .. } | Update::SeqRemoveAll { values, .. } => {
            collect_named_variants_from_expr(values, variants);
        }
        Update::Conditional {
            condition,
            then_updates,
            else_updates,
        } => {
            collect_named_variants_from_expr(condition, variants);
            for update in then_updates {
                collect_named_variants_from_update(update, variants);
            }
            for update in else_updates {
                collect_named_variants_from_update(update, variants);
            }
        }
        Update::ForEach { over, updates, .. } => {
            collect_named_variants_from_expr(over, variants);
            for update in updates {
                collect_named_variants_from_update(update, variants);
            }
        }
        Update::Increment { .. } | Update::Decrement { .. } | Update::SeqPopFront { .. } => {}
    }
}

fn collect_named_variants_from_expr(
    expr: &Expr,
    variants: &mut BTreeMap<String, BTreeSet<String>>,
) {
    match expr {
        Expr::NamedVariant { enum_name, variant } => {
            variants
                .entry(enum_name.clone())
                .or_default()
                .insert(variant.clone());
        }
        Expr::SeqLiteral(items) | Expr::And(items) | Expr::Or(items) => {
            for item in items {
                collect_named_variants_from_expr(item, variants);
            }
        }
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_named_variants_from_expr(condition, variants);
            collect_named_variants_from_expr(then_expr, variants);
            collect_named_variants_from_expr(else_expr, variants);
        }
        Expr::Not(inner)
        | Expr::Len(inner)
        | Expr::Head(inner)
        | Expr::SeqElements(inner)
        | Expr::MapKeys(inner)
        | Expr::Some(inner) => collect_named_variants_from_expr(inner, variants),
        Expr::Eq(left, right)
        | Expr::Neq(left, right)
        | Expr::Add(left, right)
        | Expr::Sub(left, right)
        | Expr::Gt(left, right)
        | Expr::Gte(left, right)
        | Expr::Lt(left, right)
        | Expr::Lte(left, right) => {
            collect_named_variants_from_expr(left, variants);
            collect_named_variants_from_expr(right, variants);
        }
        Expr::Contains { collection, value }
        | Expr::SeqStartsWith {
            seq: collection,
            prefix: value,
        } => {
            collect_named_variants_from_expr(collection, variants);
            collect_named_variants_from_expr(value, variants);
        }
        Expr::MapContainsKey { map, key } | Expr::MapGet { map, key } => {
            collect_named_variants_from_expr(map, variants);
            collect_named_variants_from_expr(key, variants);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_named_variants_from_expr(arg, variants);
            }
        }
        Expr::Quantified { over, body, .. } => {
            collect_named_variants_from_expr(over, variants);
            collect_named_variants_from_expr(body, variants);
        }
        Expr::Bool(_)
        | Expr::U64(_)
        | Expr::String(_)
        | Expr::EmptySet
        | Expr::EmptyMap
        | Expr::CurrentPhase
        | Expr::Phase(_)
        | Expr::Field(_)
        | Expr::Binding(_)
        | Expr::Variant(_)
        | Expr::None => {}
    }
}

fn generated_accessor_name(value: &str) -> String {
    let mut out = String::new();
    let mut previous_is_sep = true;

    for ch in value.chars() {
        if ch == '_' || ch == '-' || ch == ' ' {
            if !previous_is_sep {
                out.push('_');
                previous_is_sep = true;
            }
            continue;
        }

        if ch.is_ascii_uppercase() {
            if !out.is_empty() && !previous_is_sep {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            previous_is_sep = false;
        } else {
            out.push(ch.to_ascii_lowercase());
            previous_is_sep = false;
        }
    }

    out.trim_matches('_').to_owned()
}

fn rust_keyword(value: &str) -> bool {
    matches!(
        value,
        "as" | "break"
            | "const"
            | "continue"
            | "crate"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "fn"
            | "for"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "Self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "type"
            | "unsafe"
            | "use"
            | "where"
            | "while"
            | "async"
            | "await"
            | "dyn"
            | "abstract"
            | "become"
            | "box"
            | "do"
            | "final"
            | "macro"
            | "override"
            | "priv"
            | "typeof"
            | "unsized"
            | "virtual"
            | "yield"
            | "try"
    )
}

#[derive(Debug, PartialEq, Eq)]
pub enum MachineSchemaError {
    DuplicateName {
        kind: &'static str,
        name: String,
    },
    GeneratedAccessorCollision {
        namespace: &'static str,
        accessor: String,
        first_name: String,
        second_name: String,
    },
    InvalidGeneratedAccessorName {
        namespace: &'static str,
        name: String,
        accessor: String,
        reason: &'static str,
    },
    EmptyName(&'static str),
    UnknownPhase {
        phase: String,
    },
    UnknownField {
        field: String,
    },
    UnknownInputVariant {
        variant: String,
    },
    UnknownSurfaceOnlyInputVariant {
        variant: String,
    },
    UnknownSignalVariant {
        variant: String,
    },
    UnknownEffectVariant {
        variant: String,
    },
    UnknownHelper {
        helper: String,
    },
    UnknownBinding {
        binding: String,
    },
    UnknownVariant {
        variant: String,
    },
    UnknownVariantField {
        variant: String,
        field: String,
    },
    UnknownNamedVariantEnum {
        enum_name: String,
    },
    UnknownNamedVariantValue {
        enum_name: String,
        variant: String,
    },
    UnknownEffectDispositionVariant {
        variant: String,
    },
    DuplicateEffectDisposition {
        variant: String,
    },
    MissingEffectDisposition {
        variant: String,
    },
    HandoffProtocolOnRoutedEffect {
        variant: String,
    },
    SurfaceOnlyInputHasTransition {
        variant: String,
        transition: String,
    },
}

impl fmt::Display for MachineSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName { kind, name } => write!(f, "duplicate {kind} name `{name}`"),
            Self::GeneratedAccessorCollision {
                namespace,
                accessor,
                first_name,
                second_name,
            } => write!(
                f,
                "generated {namespace} accessor `{accessor}` collides for `{first_name}` and `{second_name}`"
            ),
            Self::InvalidGeneratedAccessorName {
                namespace,
                name,
                accessor,
                reason,
            } => write!(
                f,
                "generated {namespace} accessor `{accessor}` for `{name}` is invalid: {reason}"
            ),
            Self::EmptyName(kind) => write!(f, "empty {kind} name"),
            Self::UnknownPhase { phase } => write!(f, "unknown phase `{phase}`"),
            Self::UnknownField { field } => write!(f, "unknown field `{field}`"),
            Self::UnknownInputVariant { variant } => {
                write!(f, "unknown input variant `{variant}`")
            }
            Self::UnknownSurfaceOnlyInputVariant { variant } => {
                write!(f, "unknown surface-only input variant `{variant}`")
            }
            Self::UnknownSignalVariant { variant } => {
                write!(f, "unknown signal variant `{variant}`")
            }
            Self::UnknownEffectVariant { variant } => {
                write!(f, "unknown effect variant `{variant}`")
            }
            Self::UnknownHelper { helper } => write!(f, "unknown helper `{helper}`"),
            Self::UnknownBinding { binding } => write!(f, "unknown binding `{binding}`"),
            Self::UnknownVariant { variant } => write!(f, "unknown variant `{variant}`"),
            Self::UnknownVariantField { variant, field } => {
                write!(f, "unknown field `{field}` on variant `{variant}`")
            }
            Self::UnknownNamedVariantEnum { enum_name } => {
                write!(f, "unknown named-variant enum `{enum_name}`")
            }
            Self::UnknownNamedVariantValue { enum_name, variant } => {
                write!(
                    f,
                    "unknown named-variant value `{variant}` for enum `{enum_name}`"
                )
            }
            Self::UnknownEffectDispositionVariant { variant } => {
                write!(
                    f,
                    "effect disposition references unknown effect variant `{variant}`"
                )
            }
            Self::DuplicateEffectDisposition { variant } => {
                write!(f, "duplicate effect disposition for variant `{variant}`")
            }
            Self::MissingEffectDisposition { variant } => {
                write!(f, "effect variant `{variant}` has no disposition rule")
            }
            Self::HandoffProtocolOnRoutedEffect { variant } => {
                write!(
                    f,
                    "effect variant `{variant}` has handoff_protocol set but disposition is Routed (use routes instead)"
                )
            }
            Self::SurfaceOnlyInputHasTransition {
                variant,
                transition,
            } => {
                write!(
                    f,
                    "surface-only input `{variant}` must not have transition `{transition}`"
                )
            }
        }
    }
}

impl std::error::Error for MachineSchemaError {}

#[cfg(test)]
mod tests {
    use crate::{MachineSchemaError, catalog::dsl::dsl_meerkat_machine as meerkat_machine};

    #[test]
    fn validates_meerkat_machine_schema() {
        let schema = meerkat_machine();

        assert_eq!(schema.machine, "MeerkatMachine");
        // DSL-generated schema uses `rust: "self" / "catalog::dsl::meerkat_machine"`
        // because it lives inside meerkat-machine-schema itself. The runtime
        // owner is anchored in meerkat-runtime via its own `machine!` invocation.
        assert_eq!(schema.rust.crate_name, "self");
        assert_eq!(schema.rust.module, "catalog::dsl::meerkat_machine");
        assert_eq!(schema.state.phase.name, "MeerkatPhase");
        assert!(
            schema
                .transitions
                .iter()
                .any(|transition| transition.name == "PrepareBindingsIdle")
        );
        assert!(
            schema
                .transitions
                .iter()
                .any(|transition| transition.name == "Destroy")
        );
        assert_eq!(schema.state.terminal_phases, vec!["Destroyed"]);
        assert_eq!(schema.validate(), Ok(()));
    }

    #[test]
    fn validates_meerkat_machine_without_peer_directory_region() {
        let schema = meerkat_machine();

        // Peer directory region was removed (unimplemented).
        assert!(
            !schema
                .transitions
                .iter()
                .any(|transition| transition.name == "RecordSendFailedAttached")
        );
        assert_eq!(schema.validate(), Ok(()));
    }

    #[test]
    fn rejects_unknown_surface_only_inputs() {
        let mut schema = meerkat_machine();
        schema.surface_only_inputs.push("DoesNotExist".into());

        assert_eq!(
            schema.validate(),
            Err(MachineSchemaError::UnknownSurfaceOnlyInputVariant {
                variant: "DoesNotExist".into(),
            })
        );
    }

    #[test]
    fn rejects_surface_only_inputs_with_transitions() {
        let mut schema = meerkat_machine();
        schema.surface_only_inputs.push("RegisterSession".into());
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.on.variant == "RegisterSession")
            .map(|transition| transition.name.clone())
            .unwrap_or_default();
        assert!(
            !transition.is_empty(),
            "register session transition should exist"
        );

        assert_eq!(
            schema.validate(),
            Err(MachineSchemaError::SurfaceOnlyInputHasTransition {
                variant: "RegisterSession".into(),
                transition,
            })
        );
    }

    #[test]
    fn rejects_generated_accessor_collisions_after_normalization() {
        let mut schema = meerkat_machine();
        schema.inputs.variants.push(crate::VariantSchema {
            name: "Foo-Bar".into(),
            fields: vec![],
        });
        schema.inputs.variants.push(crate::VariantSchema {
            name: "foo_bar".into(),
            fields: vec![],
        });

        assert_eq!(
            schema.validate(),
            Err(MachineSchemaError::GeneratedAccessorCollision {
                namespace: "input",
                accessor: "foo_bar".into(),
                first_name: "Foo-Bar".into(),
                second_name: "foo_bar".into(),
            })
        );
    }

    #[test]
    fn rejects_generated_accessor_names_that_become_rust_keywords() {
        let mut schema = meerkat_machine();
        schema.signals.variants.push(crate::VariantSchema {
            name: "type".into(),
            fields: vec![],
        });

        assert_eq!(
            schema.validate(),
            Err(MachineSchemaError::InvalidGeneratedAccessorName {
                namespace: "signal",
                name: "type".into(),
                accessor: "type".into(),
                reason: "normalizes to a reserved Rust keyword",
            })
        );
    }
}
