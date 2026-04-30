use crate::identity::{
    EffectVariantId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, MachineId,
    NamedTypeBinding, NamedTypeId, PhaseId, ProtocolId, RustTypeAtom, SignalVariantId,
    TransitionId,
};
use indexmap::{IndexMap, IndexSet};
use std::fmt;

const NATIVE_MOB_MACHINE_HELPERS: &[&str] = &[
    "mob_machine_frame_node_status_after_admit",
    "mob_machine_frame_ready_queue_after_admit",
    "mob_machine_frame_node_status_after_terminal",
    "mob_machine_frame_ready_queue_after_terminal",
    "mob_machine_node_terminal",
    "mob_machine_step_status_from_frame_node_status",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineSchema {
    pub machine: MachineId,
    pub version: u32,
    pub rust: RustBinding,
    pub state: StateSchema,
    pub inputs: EnumSchema,
    pub surface_only_inputs: Vec<InputVariantId>,
    /// Inputs that are part of the DSL alphabet but intentionally internal to
    /// machine/composition drivers rather than public runtime command surfaces.
    pub runtime_internal_inputs: Vec<InputVariantId>,
    pub signals: EnumSchema,
    pub effects: EnumSchema,
    pub helpers: Vec<HelperSchema>,
    pub derived: Vec<HelperSchema>,
    pub invariants: Vec<InvariantSchema>,
    pub transitions: Vec<TransitionSchema>,
    pub effect_dispositions: Vec<EffectDispositionRule>,
    /// Authoritative Rust-type mapping for every `TypeRef::Named(NamedTypeId)`
    /// referenced in this schema. The codegen consults this table rather
    /// than matching on the type's name; a `NamedTypeId` referenced
    /// without a matching binding is a schema-construction error.
    pub named_types: Vec<NamedTypeBinding>,
    /// Override the CI step_limit for individual machine TLC verification.
    /// Machines with many state fields (e.g. a rich MobMachine flow/work region)
    /// may need a lower limit to keep CI verification tractable. `None` uses the
    /// codegen default (6 for CI, 8 for deep).
    pub ci_step_limit: Option<u32>,
}

impl MachineSchema {
    /// Look up the authoritative Rust binding for a named type referenced
    /// by this schema. Returns `None` if the type is unbound.
    pub fn named_type_binding(&self, name: &NamedTypeId) -> Option<&NamedTypeBinding> {
        self.named_types
            .iter()
            .find(|binding| binding.name == *name)
    }
}

impl MachineSchema {
    pub fn validate(&self) -> Result<(), MachineSchemaError> {
        let phase_names = self.state.phase.variants_by_name()?;
        let input_variants = self.inputs.variants_by_name()?;
        let surface_only_inputs = unique_names(
            self.surface_only_inputs.iter().map(AsRef::as_ref),
            "surface-only input",
        )?;
        let _runtime_internal_inputs = unique_names(
            self.runtime_internal_inputs.iter().map(AsRef::as_ref),
            "runtime-internal input",
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

        if !phase_names.contains(self.state.init.phase.as_str()) {
            return Err(MachineSchemaError::UnknownPhase {
                phase: self.state.init.phase.as_str().to_owned(),
            });
        }

        for terminal in &self.state.terminal_phases {
            if !phase_names.contains(terminal.as_str()) {
                return Err(MachineSchemaError::UnknownPhase {
                    phase: terminal.as_str().to_owned(),
                });
            }
        }

        for initializer in &self.state.init.fields {
            if !field_names.contains(initializer.field.as_str()) {
                return Err(MachineSchemaError::UnknownField {
                    field: initializer.field.as_str().to_owned(),
                });
            }
        }

        for surface_only_input in &self.surface_only_inputs {
            if !input_variants
                .iter()
                .any(|variant| *variant == surface_only_input.as_str())
            {
                return Err(MachineSchemaError::UnknownSurfaceOnlyInputVariant {
                    variant: surface_only_input.as_str().to_owned(),
                });
            }
        }

        for runtime_internal_input in &self.runtime_internal_inputs {
            if !input_variants
                .iter()
                .any(|variant| *variant == runtime_internal_input.as_str())
            {
                return Err(MachineSchemaError::UnknownRuntimeInternalInputVariant {
                    variant: runtime_internal_input.as_str().to_owned(),
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
            if transition.name.as_str().is_empty() {
                return Err(MachineSchemaError::EmptyName("transition"));
            }
            if !transition_names.insert(transition.name.as_str()) {
                return Err(MachineSchemaError::DuplicateName {
                    kind: "transition",
                    name: transition.name.as_str().to_owned(),
                });
            }
            for from in &transition.from {
                if !phase_names.contains(from.as_str()) {
                    return Err(MachineSchemaError::UnknownPhase {
                        phase: from.as_str().to_owned(),
                    });
                }
            }
            match &transition.on {
                TriggerMatch::Input { variant, .. }
                    if !input_variants.contains(variant.as_str()) =>
                {
                    return Err(MachineSchemaError::UnknownInputVariant {
                        variant: variant.as_str().to_owned(),
                    });
                }
                TriggerMatch::Signal { variant, .. }
                    if !signal_variants.contains(variant.as_str()) =>
                {
                    return Err(MachineSchemaError::UnknownSignalVariant {
                        variant: variant.as_str().to_owned(),
                    });
                }
                _ => {}
            }
            if let TriggerMatch::Input { variant, .. } = &transition.on
                && surface_only_inputs.contains(variant.as_str())
            {
                return Err(MachineSchemaError::SurfaceOnlyInputHasTransition {
                    variant: variant.as_str().to_owned(),
                    transition: transition.name.as_str().to_owned(),
                });
            }
            if !phase_names.contains(transition.to.as_str()) {
                return Err(MachineSchemaError::UnknownPhase {
                    phase: transition.to.as_str().to_owned(),
                });
            }

            let bindings = unique_names(
                transition.on.bindings().iter().map(AsRef::as_ref),
                "transition binding",
            )?;

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
                if !effect_variants.contains(effect.variant.as_str()) {
                    return Err(MachineSchemaError::UnknownEffectVariant {
                        variant: effect.variant.as_str().to_owned(),
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

        // Validate named-type bindings: every `TypeRef::Named(id)` referenced
        // anywhere in the schema must have a matching binding in `named_types`,
        // and bindings must be unique by name. This is the authoritative
        // single source of truth for how the codegen lowers named types —
        // there is no name-based fallback.
        let mut seen_bindings: IndexSet<&str> = IndexSet::new();
        for binding in &self.named_types {
            if !seen_bindings.insert(binding.name.as_str()) {
                return Err(MachineSchemaError::DuplicateNamedTypeBinding {
                    name: binding.name.as_str().to_owned(),
                });
            }
            validate_named_type_binding_payload(binding)?;
        }
        {
            let mut referenced: IndexSet<String> = IndexSet::new();
            collect_named_type_references_machine(self, &mut referenced);
            for name in &referenced {
                if !seen_bindings.contains(name.as_str()) {
                    return Err(MachineSchemaError::MissingNamedTypeBinding { name: name.clone() });
                }
            }
        }
        validate_string_enum_named_variants_machine(self)?;

        // Validate effect dispositions: every rule must reference a known effect variant,
        // no duplicates, and when dispositions are present every effect must be covered.
        if !self.effect_dispositions.is_empty() {
            let mut disposed_variants: IndexSet<&str> = IndexSet::new();
            for rule in &self.effect_dispositions {
                if !effect_variants.contains(rule.effect_variant.as_str()) {
                    return Err(MachineSchemaError::UnknownEffectDispositionVariant {
                        variant: rule.effect_variant.as_str().to_owned(),
                    });
                }
                if !disposed_variants.insert(rule.effect_variant.as_str()) {
                    return Err(MachineSchemaError::DuplicateEffectDisposition {
                        variant: rule.effect_variant.as_str().to_owned(),
                    });
                }
                if rule.handoff_protocol.is_some()
                    && matches!(rule.disposition, EffectDisposition::Routed { .. })
                {
                    return Err(MachineSchemaError::HandoffProtocolOnRoutedEffect {
                        variant: rule.effect_variant.as_str().to_owned(),
                    });
                }
            }
            for variant in &effect_variants {
                if !disposed_variants.contains(*variant) {
                    return Err(MachineSchemaError::MissingEffectDisposition {
                        variant: (*variant).to_owned(),
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
    Routed { consumer_machines: Vec<MachineId> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectDispositionRule {
    pub effect_variant: EffectVariantId,
    pub disposition: EffectDisposition,
    /// When set, this effect participates in an owner-handoff protocol.
    /// The named protocol must be declared as an `EffectHandoffProtocol`
    /// in every composition that includes this machine.
    /// Only meaningful for `Local` or `External` dispositions.
    pub handoff_protocol: Option<ProtocolId>,
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
    pub terminal_phases: Vec<PhaseId>,
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
    pub phase: PhaseId,
    pub fields: Vec<FieldInit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInit {
    pub field: FieldId,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumSchema {
    pub name: String,
    pub variants: Vec<VariantSchema>,
}

impl EnumSchema {
    pub(crate) fn variants_by_name(&self) -> Result<IndexSet<&str>, MachineSchemaError> {
        let mut names: IndexSet<&str> = IndexSet::new();
        for variant in &self.variants {
            if variant.name.as_str().is_empty() {
                return Err(MachineSchemaError::EmptyName("variant"));
            }
            if !names.insert(variant.name.as_str()) {
                return Err(MachineSchemaError::DuplicateName {
                    kind: "variant",
                    name: variant.name.as_str().to_owned(),
                });
            }
            unique_names(
                variant.fields.iter().map(|field| field.name.as_str()),
                "variant field",
            )?;
        }
        Ok(names)
    }

    pub fn variant_named(
        &self,
        name: impl AsRef<str>,
    ) -> Result<&VariantSchema, MachineSchemaError> {
        let name = name.as_ref();
        self.variants
            .iter()
            .find(|variant| variant.name.as_str() == name)
            .ok_or_else(|| MachineSchemaError::UnknownVariant {
                variant: name.to_owned(),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantSchema {
    pub name: EnumVariantId,
    pub fields: Vec<FieldSchema>,
}

impl VariantSchema {
    pub fn field_named(&self, name: impl AsRef<str>) -> Result<&FieldSchema, MachineSchemaError> {
        let name = name.as_ref();
        self.fields
            .iter()
            .find(|field| field.name.as_str() == name)
            .ok_or_else(|| MachineSchemaError::UnknownVariantField {
                variant: self.name.as_str().to_owned(),
                field: name.to_owned(),
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSchema {
    pub name: FieldId,
    pub ty: TypeRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeRef {
    Bool,
    U32,
    U64,
    String,
    Named(NamedTypeId),
    Enum(EnumTypeId),
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
    pub name: TransitionId,
    pub from: Vec<PhaseId>,
    pub on: TriggerMatch,
    pub guards: Vec<Guard>,
    pub updates: Vec<Update>,
    pub to: PhaseId,
    pub emit: Vec<EffectEmit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerKind {
    Input,
    Signal,
}

/// Typed trigger match for a transition — the variant ID is carried at the
/// right level of the sum discriminator.
///
/// Each transition fires on either an input or a signal; the `TriggerMatch`
/// sum threads the correct typed identity through each arm so there is no
/// stringly-typed `variant` field anywhere in the schema. The DSL macro
/// emits the concrete arm at expansion time — `Input { variant:
/// InputVariantId::parse(...) }` or `Signal { variant:
/// SignalVariantId::parse(...) }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TriggerMatch {
    Input {
        variant: InputVariantId,
        bindings: Vec<FieldId>,
    },
    Signal {
        variant: SignalVariantId,
        bindings: Vec<FieldId>,
    },
}

impl TriggerMatch {
    /// Structural trigger kind (convenience projection for validators that
    /// still consult the `kind`/`variant` split).
    pub fn kind(&self) -> TriggerKind {
        match self {
            Self::Input { .. } => TriggerKind::Input,
            Self::Signal { .. } => TriggerKind::Signal,
        }
    }

    /// Untyped slug accessor for the matched variant. Use [`TriggerMatch`]
    /// directly when you need the typed identity.
    pub fn variant_str(&self) -> &str {
        match self {
            Self::Input { variant, .. } => variant.as_str(),
            Self::Signal { variant, .. } => variant.as_str(),
        }
    }

    /// Typed bindings introduced by the destructure pattern.
    pub fn bindings(&self) -> &[FieldId] {
        match self {
            Self::Input { bindings, .. } | Self::Signal { bindings, .. } => bindings,
        }
    }
}

pub type InputMatch = TriggerMatch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Guard {
    pub name: String,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectEmit {
    pub variant: EffectVariantId,
    pub fields: IndexMap<FieldId, Expr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    Assign {
        field: FieldId,
        expr: Expr,
    },
    Increment {
        field: FieldId,
        amount: u64,
    },
    Decrement {
        field: FieldId,
        amount: u64,
    },
    MapInsert {
        field: FieldId,
        key: Expr,
        value: Expr,
    },
    MapIncrement {
        field: FieldId,
        key: Expr,
        amount: u64,
    },
    MapDecrement {
        field: FieldId,
        key: Expr,
        amount: u64,
    },
    MapRemove {
        field: FieldId,
        key: Expr,
    },
    SetInsert {
        field: FieldId,
        value: Expr,
    },
    SetRemove {
        field: FieldId,
        value: Expr,
    },
    SeqAppend {
        field: FieldId,
        value: Expr,
    },
    SeqPrepend {
        field: FieldId,
        values: Expr,
    },
    SeqPopFront {
        field: FieldId,
    },
    SeqRemoveValue {
        field: FieldId,
        value: Expr,
    },
    SeqRemoveAll {
        field: FieldId,
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
        phase_names: &IndexSet<&str>,
        field_names: &IndexSet<&str>,
        input_variants: &IndexSet<&str>,
        signal_variants: &IndexSet<&str>,
        effect_variants: &IndexSet<&str>,
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
                        field: field.as_str().to_owned(),
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
                        field: field.as_str().to_owned(),
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
                        field: field.as_str().to_owned(),
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
                        field: field.as_str().to_owned(),
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
                        field: field.as_str().to_owned(),
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
                        field: field.as_str().to_owned(),
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
        enum_name: EnumTypeId,
        variant: EnumVariantId,
    },
    EmptySet,
    EmptyMap,
    SeqLiteral(Vec<Expr>),
    CurrentPhase,
    Phase(PhaseId),
    Field(FieldId),
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
        phase_names: &IndexSet<&str>,
        field_names: &IndexSet<&str>,
        input_variants: &IndexSet<&str>,
        signal_variants: &IndexSet<&str>,
        effect_variants: &IndexSet<&str>,
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
                if !phase_names.contains(phase.as_str()) {
                    return Err(MachineSchemaError::UnknownPhase {
                        phase: phase.as_str().to_owned(),
                    });
                }
            }
            Self::Field(field) => {
                if !field_names.contains(field.as_str()) {
                    return Err(MachineSchemaError::UnknownField {
                        field: field.as_str().to_owned(),
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
                if !input_variants.contains(variant.as_str())
                    && !signal_variants.contains(variant.as_str())
                    && !effect_variants.contains(variant.as_str())
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
                if !helper_names.contains(helper.as_str())
                    && !NATIVE_MOB_MACHINE_HELPERS.contains(&helper.as_str())
                {
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

fn validate_named_type_binding_payload(
    binding: &NamedTypeBinding,
) -> Result<(), MachineSchemaError> {
    let RustTypeAtom::StringEnum { variants } = &binding.rust else {
        return Ok(());
    };
    if variants.is_empty() {
        return Err(MachineSchemaError::InvalidStringEnumBinding {
            name: binding.name.as_str().to_owned(),
            reason: "must define at least one variant".to_owned(),
        });
    }

    let mut seen_values: IndexSet<&str> = IndexSet::new();
    let mut seen_rust_idents: IndexMap<String, &str> = IndexMap::new();
    for variant in variants {
        let raw = variant.as_str();
        if !seen_values.insert(raw) {
            return Err(MachineSchemaError::InvalidStringEnumBinding {
                name: binding.name.as_str().to_owned(),
                reason: format!("defines duplicate variant `{raw}`"),
            });
        }

        let rust_identifier = string_enum_variant_rust_ident(raw);
        if let Some(first) = seen_rust_idents.get(&rust_identifier) {
            return Err(MachineSchemaError::InvalidStringEnumBinding {
                name: binding.name.as_str().to_owned(),
                reason: format!(
                    "variants `{first}` and `{raw}` sanitize to duplicate Rust identifier `{rust_identifier}`"
                ),
            });
        }
        seen_rust_idents.insert(rust_identifier, raw);
    }

    Ok(())
}

fn string_enum_variant_rust_ident(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

/// Recursively collect every `NamedTypeId` slug referenced by a type tree.
fn collect_named_type_references_type(ty: &TypeRef, out: &mut IndexSet<String>) {
    match ty {
        TypeRef::Bool | TypeRef::U32 | TypeRef::U64 | TypeRef::String | TypeRef::Enum(_) => {}
        TypeRef::Named(id) => {
            out.insert(id.as_str().to_owned());
        }
        TypeRef::Option(inner) | TypeRef::Set(inner) | TypeRef::Seq(inner) => {
            collect_named_type_references_type(inner, out);
        }
        TypeRef::Map(key, value) => {
            collect_named_type_references_type(key, out);
            collect_named_type_references_type(value, out);
        }
    }
}

/// Collect every named-type reference anywhere in a `MachineSchema`'s typed
/// surface: state fields, variant fields (inputs/signals/effects),
/// helpers + derived parameters and returns.
pub(crate) fn collect_named_type_references_machine(
    schema: &MachineSchema,
    out: &mut IndexSet<String>,
) {
    for field in &schema.state.fields {
        collect_named_type_references_type(&field.ty, out);
    }
    for enum_schema in [&schema.inputs, &schema.signals, &schema.effects] {
        for variant in &enum_schema.variants {
            for field in &variant.fields {
                collect_named_type_references_type(&field.ty, out);
            }
        }
    }
    for helper in schema.helpers.iter().chain(schema.derived.iter()) {
        for param in &helper.params {
            collect_named_type_references_type(&param.ty, out);
        }
        collect_named_type_references_type(&helper.returns, out);
    }
}

fn validate_string_enum_named_variants_machine(
    schema: &MachineSchema,
) -> Result<(), MachineSchemaError> {
    for init in &schema.state.init.fields {
        validate_string_enum_named_variants_expr(schema, &init.expr)?;
    }
    for invariant in &schema.invariants {
        validate_string_enum_named_variants_expr(schema, &invariant.expr)?;
    }
    for helper in schema.helpers.iter().chain(schema.derived.iter()) {
        validate_string_enum_named_variants_expr(schema, &helper.body)?;
    }
    for transition in &schema.transitions {
        for guard in &transition.guards {
            validate_string_enum_named_variants_expr(schema, &guard.expr)?;
        }
        for update in &transition.updates {
            validate_string_enum_named_variants_update(schema, update)?;
        }
        for effect in &transition.emit {
            for expr in effect.fields.values() {
                validate_string_enum_named_variants_expr(schema, expr)?;
            }
        }
    }
    Ok(())
}

fn validate_string_enum_named_variants_update(
    schema: &MachineSchema,
    update: &Update,
) -> Result<(), MachineSchemaError> {
    match update {
        Update::Assign { expr, .. } => validate_string_enum_named_variants_expr(schema, expr)?,
        Update::Increment { .. } | Update::Decrement { .. } | Update::SeqPopFront { .. } => {}
        Update::MapInsert { key, value, .. } => {
            validate_string_enum_named_variants_expr(schema, key)?;
            validate_string_enum_named_variants_expr(schema, value)?;
        }
        Update::MapIncrement { key, .. }
        | Update::MapDecrement { key, .. }
        | Update::MapRemove { key, .. } => validate_string_enum_named_variants_expr(schema, key)?,
        Update::SetInsert { value, .. }
        | Update::SetRemove { value, .. }
        | Update::SeqAppend { value, .. }
        | Update::SeqRemoveValue { value, .. } => {
            validate_string_enum_named_variants_expr(schema, value)?;
        }
        Update::SeqPrepend { values, .. } | Update::SeqRemoveAll { values, .. } => {
            validate_string_enum_named_variants_expr(schema, values)?;
        }
        Update::Conditional {
            condition,
            then_updates,
            else_updates,
        } => {
            validate_string_enum_named_variants_expr(schema, condition)?;
            for nested in then_updates.iter().chain(else_updates.iter()) {
                validate_string_enum_named_variants_update(schema, nested)?;
            }
        }
        Update::ForEach { over, updates, .. } => {
            validate_string_enum_named_variants_expr(schema, over)?;
            for nested in updates {
                validate_string_enum_named_variants_update(schema, nested)?;
            }
        }
    }
    Ok(())
}

fn validate_string_enum_named_variants_expr(
    schema: &MachineSchema,
    expr: &Expr,
) -> Result<(), MachineSchemaError> {
    match expr {
        Expr::NamedVariant { enum_name, variant } => {
            let Ok(named_type_name) = NamedTypeId::parse(enum_name.as_str()) else {
                return Ok(());
            };
            if let Some(NamedTypeBinding {
                rust: RustTypeAtom::StringEnum { variants },
                ..
            }) = schema.named_type_binding(&named_type_name)
                && !variants.iter().any(|allowed| allowed == variant)
            {
                return Err(MachineSchemaError::UnknownStringEnumVariant {
                    enum_name: enum_name.as_str().to_owned(),
                    variant: variant.as_str().to_owned(),
                });
            }
        }
        Expr::SeqLiteral(items) | Expr::And(items) | Expr::Or(items) => {
            for item in items {
                validate_string_enum_named_variants_expr(schema, item)?;
            }
        }
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            validate_string_enum_named_variants_expr(schema, condition)?;
            validate_string_enum_named_variants_expr(schema, then_expr)?;
            validate_string_enum_named_variants_expr(schema, else_expr)?;
        }
        Expr::Not(inner)
        | Expr::Len(inner)
        | Expr::Head(inner)
        | Expr::MapKeys(inner)
        | Expr::SeqElements(inner)
        | Expr::Some(inner) => validate_string_enum_named_variants_expr(schema, inner)?,
        Expr::Eq(left, right)
        | Expr::Neq(left, right)
        | Expr::Add(left, right)
        | Expr::Sub(left, right)
        | Expr::Gt(left, right)
        | Expr::Gte(left, right)
        | Expr::Lt(left, right)
        | Expr::Lte(left, right) => {
            validate_string_enum_named_variants_expr(schema, left)?;
            validate_string_enum_named_variants_expr(schema, right)?;
        }
        Expr::Contains { collection, value } => {
            validate_string_enum_named_variants_expr(schema, collection)?;
            validate_string_enum_named_variants_expr(schema, value)?;
        }
        Expr::MapContainsKey { map, key } | Expr::MapGet { map, key } => {
            validate_string_enum_named_variants_expr(schema, map)?;
            validate_string_enum_named_variants_expr(schema, key)?;
        }
        Expr::SeqStartsWith { seq, prefix } => {
            validate_string_enum_named_variants_expr(schema, seq)?;
            validate_string_enum_named_variants_expr(schema, prefix)?;
        }
        Expr::Call { args, .. } => {
            for arg in args {
                validate_string_enum_named_variants_expr(schema, arg)?;
            }
        }
        Expr::Quantified { over, body, .. } => {
            validate_string_enum_named_variants_expr(schema, over)?;
            validate_string_enum_named_variants_expr(schema, body)?;
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
    Ok(())
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

#[derive(Debug, PartialEq, Eq)]
pub enum MachineSchemaError {
    DuplicateName { kind: &'static str, name: String },
    EmptyName(&'static str),
    UnknownPhase { phase: String },
    UnknownField { field: String },
    UnknownInputVariant { variant: String },
    UnknownSurfaceOnlyInputVariant { variant: String },
    UnknownRuntimeInternalInputVariant { variant: String },
    UnknownSignalVariant { variant: String },
    UnknownEffectVariant { variant: String },
    UnknownHelper { helper: String },
    UnknownBinding { binding: String },
    UnknownVariant { variant: String },
    UnknownVariantField { variant: String, field: String },
    UnknownEffectDispositionVariant { variant: String },
    DuplicateEffectDisposition { variant: String },
    MissingEffectDisposition { variant: String },
    HandoffProtocolOnRoutedEffect { variant: String },
    SurfaceOnlyInputHasTransition { variant: String, transition: String },
    DuplicateNamedTypeBinding { name: String },
    MissingNamedTypeBinding { name: String },
    UnknownStringEnumVariant { enum_name: String, variant: String },
    InvalidStringEnumBinding { name: String, reason: String },
}

impl fmt::Display for MachineSchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateName { kind, name } => write!(f, "duplicate {kind} name `{name}`"),
            Self::EmptyName(kind) => write!(f, "empty {kind} name"),
            Self::UnknownPhase { phase } => write!(f, "unknown phase `{phase}`"),
            Self::UnknownField { field } => write!(f, "unknown field `{field}`"),
            Self::UnknownInputVariant { variant } => {
                write!(f, "unknown input variant `{variant}`")
            }
            Self::UnknownSurfaceOnlyInputVariant { variant } => {
                write!(f, "unknown surface-only input variant `{variant}`")
            }
            Self::UnknownRuntimeInternalInputVariant { variant } => {
                write!(f, "unknown runtime-internal input variant `{variant}`")
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
            Self::DuplicateNamedTypeBinding { name } => {
                write!(f, "duplicate named-type binding for `{name}`")
            }
            Self::MissingNamedTypeBinding { name } => {
                write!(
                    f,
                    "named type `{name}` is referenced by this schema but has no NamedTypeBinding entry in `named_types`"
                )
            }
            Self::UnknownStringEnumVariant { enum_name, variant } => {
                write!(
                    f,
                    "string enum `{enum_name}` does not define variant `{variant}`"
                )
            }
            Self::InvalidStringEnumBinding { name, reason } => {
                write!(
                    f,
                    "invalid string enum named-type binding `{name}`: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for MachineSchemaError {}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use crate::identity::{EnumTypeId, EnumVariantId, NamedTypeId};
    use crate::{
        Expr, MachineSchema, MachineSchemaError, NamedTypeBinding, RustTypeAtom, Update,
        catalog::dsl::dsl_meerkat_machine as meerkat_machine,
    };

    #[test]
    fn validates_meerkat_machine_schema() {
        let schema = meerkat_machine();

        assert_eq!(schema.machine.as_str(), "MeerkatMachine");
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
                .any(|transition| transition.name.as_str() == "PrepareBindingsIdle")
        );
        assert!(
            schema
                .transitions
                .iter()
                .any(|transition| transition.name.as_str() == "Destroy")
        );
        assert_eq!(
            schema
                .state
                .terminal_phases
                .iter()
                .map(|phase| phase.as_str().to_owned())
                .collect::<Vec<_>>(),
            vec!["Destroyed".to_owned()]
        );
        assert_eq!(schema.validate(), Ok(()));
    }

    fn replace_register_op_status_update(schema: &mut MachineSchema, status: &str) {
        let transition = schema
            .transitions
            .iter_mut()
            .find(|transition| transition.name.as_str() == "RegisterOpIdle")
            .expect("RegisterOpIdle transition");
        let update = transition
            .updates
            .iter_mut()
            .find(|update| {
                matches!(
                    update,
                    Update::MapInsert { field, .. } if field.as_str() == "op_statuses"
                )
            })
            .expect("op_statuses update");
        let Update::MapInsert { value, .. } = update else {
            panic!("op_statuses update must be a map insert");
        };
        *value = Expr::NamedVariant {
            enum_name: EnumTypeId::parse("OperationStatus").expect("enum type slug"),
            variant: EnumVariantId::parse(status).expect("enum variant slug"),
        };
    }

    fn string_enum_binding(name: &str, variants: &[&str]) -> NamedTypeBinding {
        NamedTypeBinding {
            name: NamedTypeId::parse(name).expect("named type slug"),
            rust: RustTypeAtom::StringEnum {
                variants: variants
                    .iter()
                    .map(|variant| EnumVariantId::parse(*variant).expect("enum variant slug"))
                    .collect(),
            },
        }
    }

    #[test]
    fn validate_rejects_unknown_string_enum_named_variant_in_transition_update() {
        let mut schema = meerkat_machine();
        replace_register_op_status_update(&mut schema, "Launched");

        let err = schema
            .validate()
            .expect_err("unknown OperationStatus variant must be rejected");
        let message = err.to_string();
        assert!(
            message.contains("OperationStatus") && message.contains("Launched"),
            "error should identify the invalid string enum variant, got: {message}"
        );
    }

    #[test]
    fn validate_rejects_empty_string_enum_named_type_binding() {
        let mut schema = meerkat_machine();
        schema
            .named_types
            .push(string_enum_binding("SyntheticStatus", &[]));

        let err = schema
            .validate()
            .expect_err("empty StringEnum bindings must be rejected");
        let message = err.to_string();
        assert!(
            message.contains("SyntheticStatus") && message.contains("at least one variant"),
            "error should identify the empty StringEnum binding, got: {message}"
        );
    }

    #[test]
    fn validate_rejects_duplicate_string_enum_named_type_variants() {
        let mut schema = meerkat_machine();
        schema
            .named_types
            .push(string_enum_binding("SyntheticStatus", &["Ready", "Ready"]));

        let err = schema
            .validate()
            .expect_err("duplicate StringEnum variants must be rejected");
        let message = err.to_string();
        assert!(
            message.contains("SyntheticStatus")
                && message.contains("Ready")
                && message.contains("duplicate"),
            "error should identify the duplicate StringEnum variant, got: {message}"
        );
    }

    #[test]
    fn validate_rejects_string_enum_named_type_variant_ident_collisions() {
        let mut schema = meerkat_machine();
        schema.named_types.push(string_enum_binding(
            "SyntheticStatus",
            &["foo-bar", "foo_bar"],
        ));

        let err = schema
            .validate()
            .expect_err("StringEnum variant Rust identifier collisions must be rejected");
        let message = err.to_string();
        assert!(
            message.contains("SyntheticStatus")
                && message.contains("foo-bar")
                && message.contains("foo_bar"),
            "error should identify colliding StringEnum variants, got: {message}"
        );
    }

    #[test]
    fn validates_meerkat_machine_without_peer_directory_region() {
        let schema = meerkat_machine();

        // Peer directory region was removed (unimplemented).
        assert!(
            !schema
                .transitions
                .iter()
                .any(|transition| transition.name.as_str() == "RecordSendFailedAttached")
        );
        assert_eq!(schema.validate(), Ok(()));
    }

    #[test]
    fn rejects_unknown_surface_only_inputs() {
        let mut schema = meerkat_machine();
        schema
            .surface_only_inputs
            .push(crate::identity::InputVariantId::parse("DoesNotExist").expect("slug"));

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
        schema
            .surface_only_inputs
            .push(crate::identity::InputVariantId::parse("RegisterSession").expect("slug"));
        let transition = schema
            .transitions
            .iter()
            .find(|transition| transition.on.variant_str() == "RegisterSession")
            .map(|transition| transition.name.as_str().to_owned())
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
}
