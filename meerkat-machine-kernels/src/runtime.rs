use std::{
    borrow::{Borrow, Cow},
    collections::{BTreeMap, BTreeSet},
    fmt,
};

use meerkat_machine_schema::{
    EffectEmit, Expr, FieldSchema, HelperSchema, MachineSchema, Quantifier, TransitionSchema,
    TriggerKind, TypeRef, Update,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

macro_rules! kernel_ident {
    ($name:ident) => {
        #[derive(
            Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Cow<'static, str>);

        impl $name {
            #[must_use]
            pub const fn new_static(value: &'static str) -> Self {
                Self(Cow::Borrowed(value))
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                self.0.as_ref()
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(Cow::Owned(value))
            }
        }

        impl From<&String> for $name {
            fn from(value: &String) -> Self {
                Self(Cow::Owned(value.clone()))
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(Cow::Owned(value.to_owned()))
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.as_str() == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.as_str() == *other
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                self.as_str() == other
            }
        }
    };
}

kernel_ident!(KernelPhase);
kernel_ident!(KernelField);
kernel_ident!(KernelInputVariant);
kernel_ident!(KernelSignalVariant);
kernel_ident!(KernelEffectVariant);
kernel_ident!(KernelTransitionName);
kernel_ident!(KernelHelperName);
kernel_ident!(KernelNamedType);
kernel_ident!(KernelEnumName);
kernel_ident!(KernelEnumVariant);

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct KernelNamedVariant {
    pub enum_name: KernelEnumName,
    pub variant: KernelEnumVariant,
}

impl KernelNamedVariant {
    #[must_use]
    pub const fn new_static(enum_name: &'static str, variant: &'static str) -> Self {
        Self {
            enum_name: KernelEnumName::new_static(enum_name),
            variant: KernelEnumVariant::new_static(variant),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum KernelValue {
    Bool(bool),
    U64(u64),
    String(String),
    Named {
        type_name: KernelNamedType,
        value: String,
    },
    NamedVariant {
        enum_name: KernelEnumName,
        variant: KernelEnumVariant,
    },
    Seq(Vec<KernelValue>),
    Set(BTreeSet<KernelValue>),
    Map(BTreeMap<KernelValue, KernelValue>),
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum KernelValueRepr {
    Bool {
        value: bool,
    },
    U64 {
        value: u64,
    },
    String {
        value: String,
    },
    Named {
        type_name: KernelNamedType,
        value: String,
    },
    NamedVariant {
        enum_name: KernelEnumName,
        variant: KernelEnumVariant,
    },
    Seq {
        items: Vec<KernelValue>,
    },
    Set {
        items: Vec<KernelValue>,
    },
    Map {
        entries: Vec<(KernelValue, KernelValue)>,
    },
    None,
}

impl From<&KernelValue> for KernelValueRepr {
    fn from(value: &KernelValue) -> Self {
        match value {
            KernelValue::Bool(value) => Self::Bool { value: *value },
            KernelValue::U64(value) => Self::U64 { value: *value },
            KernelValue::String(value) => Self::String {
                value: value.clone(),
            },
            KernelValue::Named { type_name, value } => Self::Named {
                type_name: type_name.clone(),
                value: value.clone(),
            },
            KernelValue::NamedVariant { enum_name, variant } => Self::NamedVariant {
                enum_name: enum_name.clone(),
                variant: variant.clone(),
            },
            KernelValue::Seq(values) => Self::Seq {
                items: values.clone(),
            },
            KernelValue::Set(values) => Self::Set {
                items: values.iter().cloned().collect(),
            },
            KernelValue::Map(values) => Self::Map {
                entries: values
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            },
            KernelValue::None => Self::None,
        }
    }
}

impl From<KernelValueRepr> for KernelValue {
    fn from(value: KernelValueRepr) -> Self {
        match value {
            KernelValueRepr::Bool { value } => Self::Bool(value),
            KernelValueRepr::U64 { value } => Self::U64(value),
            KernelValueRepr::String { value } => Self::String(value),
            KernelValueRepr::Named { type_name, value } => Self::Named { type_name, value },
            KernelValueRepr::NamedVariant { enum_name, variant } => {
                Self::NamedVariant { enum_name, variant }
            }
            KernelValueRepr::Seq { items } => Self::Seq(items),
            KernelValueRepr::Set { items } => Self::Set(items.into_iter().collect()),
            KernelValueRepr::Map { entries } => Self::Map(entries.into_iter().collect()),
            KernelValueRepr::None => Self::None,
        }
    }
}

impl Serialize for KernelValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        KernelValueRepr::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for KernelValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(KernelValueRepr::deserialize(deserializer)?.into())
    }
}

fn option_some(value: KernelValue) -> KernelValue {
    KernelValue::Map(BTreeMap::from([(
        KernelValue::String("value".to_string()),
        value,
    )]))
}

fn option_map_inner(value: &KernelValue) -> Option<&KernelValue> {
    let KernelValue::Map(entries) = value else {
        return None;
    };
    if entries.len() != 1 {
        return None;
    }
    entries.get(&KernelValue::String("value".to_string()))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelState {
    pub phase: KernelPhase,
    pub fields: KernelFields,
}

impl KernelState {
    #[must_use]
    pub fn field(&self, field: &KernelField) -> Option<&KernelValue> {
        self.fields.get(field.as_str())
    }

    #[must_use]
    pub fn phase_is(&self, phase: &KernelPhase) -> bool {
        &self.phase == phase
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelInput {
    pub variant: KernelInputVariant,
    pub fields: KernelFields,
}

impl KernelInput {
    #[must_use]
    pub fn new(variant: KernelInputVariant, fields: KernelFields) -> Self {
        Self { variant, fields }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelSignal {
    pub variant: KernelSignalVariant,
    pub fields: KernelFields,
}

impl KernelSignal {
    #[must_use]
    pub fn new(variant: KernelSignalVariant, fields: KernelFields) -> Self {
        Self { variant, fields }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelEffect {
    pub variant: KernelEffectVariant,
    pub fields: KernelFields,
}

impl KernelEffect {
    #[must_use]
    pub fn field(&self, field: &KernelField) -> Option<&KernelValue> {
        self.fields.get(field.as_str())
    }

    #[must_use]
    pub fn variant_is(&self, variant: &KernelEffectVariant) -> bool {
        &self.variant == variant
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionOutcome {
    pub transition: KernelTransitionName,
    pub next_state: KernelState,
    pub effects: Vec<KernelEffect>,
}

pub type KernelFields = BTreeMap<KernelField, KernelValue>;

#[derive(Debug, Clone, Copy)]
enum TriggerVariantRef<'a> {
    Input(&'a KernelInputVariant),
    Signal(&'a KernelSignalVariant),
}

impl<'a> TriggerVariantRef<'a> {
    fn as_str(self) -> &'a str {
        match self {
            Self::Input(variant) => variant.as_str(),
            Self::Signal(variant) => variant.as_str(),
        }
    }

    fn kind(self) -> TriggerKind {
        match self {
            Self::Input(_) => TriggerKind::Input,
            Self::Signal(_) => TriggerKind::Signal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransitionRefusal {
    #[error("unknown input variant `{variant}` for machine `{machine}`")]
    UnknownInputVariant { machine: String, variant: String },
    #[error("unknown signal variant `{variant}` for machine `{machine}`")]
    UnknownSignalVariant { machine: String, variant: String },
    #[error("invalid input payload for machine `{machine}` variant `{variant}`: {reason}")]
    InvalidInputPayload {
        machine: String,
        variant: String,
        reason: String,
    },
    #[error("invalid signal payload for machine `{machine}` variant `{variant}`: {reason}")]
    InvalidSignalPayload {
        machine: String,
        variant: String,
        reason: String,
    },
    #[error("no matching transition for machine `{machine}` in phase `{phase}` on `{variant}`")]
    NoMatchingTransition {
        machine: String,
        phase: String,
        variant: String,
    },
    #[error(
        "ambiguous transitions for machine `{machine}` in phase `{phase}` on `{variant}`: {transitions:?}"
    )]
    AmbiguousTransition {
        machine: String,
        phase: String,
        variant: String,
        transitions: Vec<String>,
    },
    #[error("evaluation error in machine `{machine}` transition `{transition}`: {reason}")]
    EvaluationError {
        machine: String,
        transition: String,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct GeneratedMachineKernel {
    schema: MachineSchema,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeValidationMode {
    Boundary,
    SchemaOwned,
}

impl TypeValidationMode {
    const fn allows_named_string_coercion(self) -> bool {
        matches!(self, Self::Boundary | Self::SchemaOwned)
    }
}

impl GeneratedMachineKernel {
    #[must_use]
    pub fn new(schema: MachineSchema) -> Self {
        Self { schema }
    }

    #[must_use]
    pub fn schema(&self) -> &MachineSchema {
        &self.schema
    }

    fn normalize_state(&self, state: &KernelState) -> Result<KernelState, TransitionRefusal> {
        let mut normalized = state.clone();
        for field in &self.schema.state.fields {
            let Some(value) = normalized.fields.get(field.name.as_str()).cloned() else {
                continue;
            };
            let normalized_value = self
                .normalize_value_for_type(&value, &field.ty, TypeValidationMode::SchemaOwned)
                .map_err(|reason| {
                    self.eval_error("<state>", format!("field `{}` {reason}", field.name))
                })?;
            normalized
                .fields
                .insert(field.name.clone().into(), normalized_value);
        }
        Ok(normalized)
    }

    fn normalize_trigger_fields(
        &self,
        fields: &KernelFields,
        trigger_kind: TriggerKind,
        variant: &str,
        trigger_schema_fields: &[FieldSchema],
    ) -> Result<KernelFields, TransitionRefusal> {
        let mut normalized = KernelFields::new();
        for field in trigger_schema_fields {
            let Some(value) = fields.get(field.name.as_str()) else {
                return Err(match trigger_kind {
                    TriggerKind::Input => TransitionRefusal::InvalidInputPayload {
                        machine: self.schema.machine.clone(),
                        variant: variant.to_owned(),
                        reason: format!("missing field `{}`", field.name),
                    },
                    TriggerKind::Signal => TransitionRefusal::InvalidSignalPayload {
                        machine: self.schema.machine.clone(),
                        variant: variant.to_owned(),
                        reason: format!("missing field `{}`", field.name),
                    },
                });
            };
            let normalized_value =
                self.normalize_value_for_type(value, &field.ty, TypeValidationMode::Boundary);
            match normalized_value {
                Ok(value) => {
                    normalized.insert(field.name.clone().into(), value);
                }
                Err(_) => {
                    return Err(match trigger_kind {
                        TriggerKind::Input => TransitionRefusal::InvalidInputPayload {
                            machine: self.schema.machine.clone(),
                            variant: variant.to_owned(),
                            reason: format!("field `{}` does not match declared type", field.name),
                        },
                        TriggerKind::Signal => TransitionRefusal::InvalidSignalPayload {
                            machine: self.schema.machine.clone(),
                            variant: variant.to_owned(),
                            reason: format!("field `{}` does not match declared type", field.name),
                        },
                    });
                }
            }
        }
        Ok(normalized)
    }

    fn normalize_value_for_type(
        &self,
        value: &KernelValue,
        ty: &TypeRef,
        mode: TypeValidationMode,
    ) -> Result<KernelValue, String> {
        match ty {
            TypeRef::Bool => match value {
                KernelValue::Bool(value) => Ok(KernelValue::Bool(*value)),
                other => Err(format!("expected bool, found {other:?}")),
            },
            TypeRef::U32 | TypeRef::U64 => match value {
                KernelValue::U64(value) => Ok(KernelValue::U64(*value)),
                other => Err(format!("expected u64, found {other:?}")),
            },
            TypeRef::String => match value {
                KernelValue::String(value) => Ok(KernelValue::String(value.clone())),
                other => Err(format!("expected string, found {other:?}")),
            },
            TypeRef::Named(name) if named_type_is_u64(name) => match value {
                KernelValue::U64(value) => Ok(KernelValue::U64(*value)),
                other => Err(format!("expected named u64 `{name}`, found {other:?}")),
            },
            TypeRef::Named(name) => {
                let has_declared_variants = self.named_type_has_declared_variants(name);
                match value {
                    KernelValue::NamedVariant { enum_name, variant }
                        if enum_name.as_str() == name
                            && self.enum_variant_is_declared(name, variant.as_str()) =>
                    {
                        Ok(KernelValue::NamedVariant {
                            enum_name: enum_name.clone(),
                            variant: variant.clone(),
                        })
                    }
                    KernelValue::Named { type_name, value } if type_name.as_str() == name => {
                        if has_declared_variants
                            && self.enum_variant_is_declared(name, value.as_str())
                        {
                            Ok(KernelValue::NamedVariant {
                                enum_name: name.clone().into(),
                                variant: value.clone().into(),
                            })
                        } else {
                            Ok(KernelValue::Named {
                                type_name: type_name.clone(),
                                value: value.clone(),
                            })
                        }
                    }
                    KernelValue::String(value) if mode.allows_named_string_coercion() => {
                        if has_declared_variants
                            && self.enum_variant_is_declared(name, value.as_str())
                        {
                            Ok(KernelValue::NamedVariant {
                                enum_name: name.clone().into(),
                                variant: value.clone().into(),
                            })
                        } else {
                            Ok(KernelValue::Named {
                                type_name: name.clone().into(),
                                value: value.clone(),
                            })
                        }
                    }
                    KernelValue::Named { type_name, .. } => Err(format!(
                        "expected named `{name}`, found named `{type_name}`"
                    )),
                    KernelValue::NamedVariant { enum_name, .. } => Err(format!(
                        "expected named `{name}`, found named variant of `{enum_name}`"
                    )),
                    other => Err(format!("expected named `{name}`, found {other:?}")),
                }
            }
            TypeRef::Enum(name) => match value {
                KernelValue::NamedVariant { enum_name, variant }
                    if enum_name.as_str() == name
                        && self.enum_variant_is_declared(name, variant.as_str()) =>
                {
                    Ok(KernelValue::NamedVariant {
                        enum_name: enum_name.clone(),
                        variant: variant.clone(),
                    })
                }
                KernelValue::NamedVariant { enum_name, variant } if enum_name.as_str() == name => {
                    Err(format!(
                        "expected declared variant of enum `{name}`, found `{variant}`"
                    ))
                }
                KernelValue::NamedVariant { enum_name, .. } => Err(format!(
                    "expected named variant of enum `{name}`, found enum `{enum_name}`"
                )),
                other => Err(format!(
                    "expected named variant of enum `{name}`, found {other:?}"
                )),
            },
            TypeRef::Option(inner_ty) => {
                if matches!(value, KernelValue::None) {
                    return Ok(KernelValue::None);
                }
                if let Some(inner) = option_map_inner(value) {
                    return self
                        .normalize_value_for_type(inner, inner_ty, mode)
                        .map(option_some);
                }
                self.normalize_value_for_type(value, inner_ty, mode)
                    .map(option_some)
            }
            TypeRef::Set(inner_ty) => match value {
                KernelValue::Set(values) => values
                    .iter()
                    .map(|value| self.normalize_value_for_type(value, inner_ty, mode))
                    .collect::<Result<BTreeSet<_>, _>>()
                    .map(KernelValue::Set),
                other => Err(format!("expected set, found {other:?}")),
            },
            TypeRef::Seq(inner_ty) => match value {
                KernelValue::Seq(values) => values
                    .iter()
                    .map(|value| self.normalize_value_for_type(value, inner_ty, mode))
                    .collect::<Result<Vec<_>, _>>()
                    .map(KernelValue::Seq),
                other => Err(format!("expected seq, found {other:?}")),
            },
            TypeRef::Map(key_ty, value_ty) => match value {
                KernelValue::Map(values) => values
                    .iter()
                    .map(|(key, value)| {
                        Ok((
                            self.normalize_value_for_type(key, key_ty, mode)?,
                            self.normalize_value_for_type(value, value_ty, mode)?,
                        ))
                    })
                    .collect::<Result<BTreeMap<_, _>, String>>()
                    .map(KernelValue::Map),
                other => Err(format!("expected map, found {other:?}")),
            },
        }
    }

    fn enum_variant_is_declared(&self, enum_name: &str, variant: &str) -> bool {
        schema_declares_named_variant(&self.schema, enum_name, variant)
    }

    fn named_type_has_declared_variants(&self, name: &str) -> bool {
        schema_declares_named_variant_family(&self.schema, name)
    }

    fn state_field_schema(&self, field: &str) -> Result<&FieldSchema, TransitionRefusal> {
        self.schema
            .state
            .fields
            .iter()
            .find(|candidate| candidate.name == field)
            .ok_or_else(|| self.eval_error("<schema>", format!("unknown state field `{field}`")))
    }

    fn map_field_types(&self, field: &str) -> Result<(&TypeRef, &TypeRef), TransitionRefusal> {
        let field_schema = self.state_field_schema(field)?;
        let TypeRef::Map(key_ty, value_ty) = &field_schema.ty else {
            return Err(self.eval_error("<schema>", format!("state field `{field}` is not a map")));
        };
        Ok((key_ty.as_ref(), value_ty.as_ref()))
    }

    fn set_field_item_type(&self, field: &str) -> Result<&TypeRef, TransitionRefusal> {
        let field_schema = self.state_field_schema(field)?;
        let TypeRef::Set(inner_ty) = &field_schema.ty else {
            return Err(self.eval_error("<schema>", format!("state field `{field}` is not a set")));
        };
        Ok(inner_ty.as_ref())
    }

    fn seq_field_item_type(&self, field: &str) -> Result<&TypeRef, TransitionRefusal> {
        let field_schema = self.state_field_schema(field)?;
        let TypeRef::Seq(inner_ty) = &field_schema.ty else {
            return Err(self.eval_error("<schema>", format!("state field `{field}` is not a seq")));
        };
        Ok(inner_ty.as_ref())
    }

    pub fn initial_state(&self) -> Result<KernelState, TransitionRefusal> {
        let mut state = KernelState {
            phase: self.schema.state.init.phase.clone().into(),
            fields: self
                .schema
                .state
                .fields
                .iter()
                .map(|field| (field.name.clone().into(), default_value_for_type(&field.ty)))
                .collect(),
        };

        for init in &self.schema.state.init.fields {
            let field_schema = self.state_field_schema(&init.field)?;
            let value = self.eval_expr(&state, &BTreeMap::new(), &init.expr, "<init>")?;
            let value = self
                .normalize_value_for_type(&value, &field_schema.ty, TypeValidationMode::SchemaOwned)
                .map_err(|reason| {
                    self.eval_error("<init>", format!("field `{}` {reason}", init.field))
                })?;
            state.fields.insert(init.field.clone().into(), value);
        }

        self.normalize_state(&state)
    }

    pub fn transition(
        &self,
        state: &KernelState,
        input: &KernelInput,
    ) -> Result<TransitionOutcome, TransitionRefusal> {
        self.transition_with_kind(
            state,
            TriggerVariantRef::Input(&input.variant),
            &input.fields,
        )
    }

    pub fn transition_signal(
        &self,
        state: &KernelState,
        signal: &KernelSignal,
    ) -> Result<TransitionOutcome, TransitionRefusal> {
        self.transition_with_kind(
            state,
            TriggerVariantRef::Signal(&signal.variant),
            &signal.fields,
        )
    }

    fn transition_with_kind(
        &self,
        state: &KernelState,
        trigger_variant: TriggerVariantRef<'_>,
        fields: &KernelFields,
    ) -> Result<TransitionOutcome, TransitionRefusal> {
        let state = self.normalize_state(state)?;
        let variant = trigger_variant.as_str();
        let trigger_kind = trigger_variant.kind();
        let trigger_schema = match trigger_kind {
            TriggerKind::Input => self.schema.inputs.variant_named(variant).map_err(|_| {
                TransitionRefusal::UnknownInputVariant {
                    machine: self.schema.machine.clone(),
                    variant: variant.to_owned(),
                }
            })?,
            TriggerKind::Signal => self.schema.signals.variant_named(variant).map_err(|_| {
                TransitionRefusal::UnknownSignalVariant {
                    machine: self.schema.machine.clone(),
                    variant: variant.to_owned(),
                }
            })?,
        };
        let fields =
            self.normalize_trigger_fields(fields, trigger_kind, variant, &trigger_schema.fields)?;

        let mut matches = Vec::new();
        for transition in &self.schema.transitions {
            if !transition
                .from
                .iter()
                .any(|phase| phase.as_str() == state.phase.as_str())
            {
                continue;
            }
            if transition.on.kind != trigger_kind || transition.on.variant != variant {
                continue;
            }

            let mut bindings = BTreeMap::new();
            let mut malformed = false;
            for binding in &transition.on.bindings {
                let Some(value) = fields.get(binding.as_str()) else {
                    malformed = true;
                    break;
                };
                bindings.insert(binding.clone(), value.clone());
            }
            if malformed {
                return Err(match trigger_kind {
                    TriggerKind::Input => TransitionRefusal::InvalidInputPayload {
                        machine: self.schema.machine.clone(),
                        variant: variant.to_owned(),
                        reason: "transition binding missing from payload".into(),
                    },
                    TriggerKind::Signal => TransitionRefusal::InvalidSignalPayload {
                        machine: self.schema.machine.clone(),
                        variant: variant.to_owned(),
                        reason: "transition binding missing from payload".into(),
                    },
                });
            }

            let guards_hold = transition.guards.iter().try_fold(true, |acc, guard| {
                let value = self.eval_expr(&state, &bindings, &guard.expr, &transition.name)?;
                let as_bool =
                    value
                        .as_bool()
                        .map_err(|reason| TransitionRefusal::EvaluationError {
                            machine: self.schema.machine.clone(),
                            transition: transition.name.clone(),
                            reason: format!("guard `{}` {reason}", guard.name),
                        })?;
                Ok::<bool, TransitionRefusal>(acc && as_bool)
            })?;

            if guards_hold {
                matches.push((transition, bindings));
            }
        }

        match matches.len() {
            0 => Err(TransitionRefusal::NoMatchingTransition {
                machine: self.schema.machine.clone(),
                phase: state.phase.to_string(),
                variant: variant.to_owned(),
            }),
            1 => {
                let Some((transition, bindings)) = matches.pop() else {
                    return Err(TransitionRefusal::NoMatchingTransition {
                        machine: self.schema.machine.clone(),
                        phase: state.phase.to_string(),
                        variant: variant.to_owned(),
                    });
                };
                self.apply_transition(&state, transition, &bindings)
            }
            _ => Err(TransitionRefusal::AmbiguousTransition {
                machine: self.schema.machine.clone(),
                phase: state.phase.to_string(),
                variant: variant.to_owned(),
                transitions: matches
                    .iter()
                    .map(|(transition, _)| transition.name.clone())
                    .collect(),
            }),
        }
    }

    fn apply_transition(
        &self,
        state: &KernelState,
        transition: &TransitionSchema,
        bindings: &BTreeMap<String, KernelValue>,
    ) -> Result<TransitionOutcome, TransitionRefusal> {
        let mut next_state = state.clone();
        for update in &transition.updates {
            self.apply_update(&mut next_state, bindings, update, &transition.name)?;
        }
        next_state.phase = transition.to.clone().into();

        let mut effects = Vec::new();
        for effect in &transition.emit {
            effects.push(self.render_effect(&next_state, bindings, effect, &transition.name)?);
        }

        Ok(TransitionOutcome {
            transition: transition.name.clone().into(),
            next_state,
            effects,
        })
    }

    pub fn evaluate_helper(
        &self,
        state: &KernelState,
        helper_name: &KernelHelperName,
        args: &KernelFields,
    ) -> Result<KernelValue, TransitionRefusal> {
        let helper = self
            .schema
            .helpers
            .iter()
            .chain(self.schema.derived.iter())
            .find(|candidate| candidate.name == helper_name.as_str())
            .ok_or_else(|| TransitionRefusal::EvaluationError {
                machine: self.schema.machine.clone(),
                transition: "<helper>".to_string(),
                reason: format!("unknown helper `{helper_name}`"),
            })?;

        let mut bindings = BTreeMap::new();
        for param in &helper.params {
            let Some(value) = args.get(param.name.as_str()) else {
                return Err(TransitionRefusal::EvaluationError {
                    machine: self.schema.machine.clone(),
                    transition: "<helper>".to_string(),
                    reason: format!("missing helper arg `{}`", param.name),
                });
            };
            let normalized = self
                .normalize_value_for_type(value, &param.ty, TypeValidationMode::Boundary)
                .map_err(|_| TransitionRefusal::EvaluationError {
                    machine: self.schema.machine.clone(),
                    transition: "<helper>".to_string(),
                    reason: format!("helper arg `{}` does not match declared type", param.name),
                })?;
            bindings.insert(param.name.clone(), normalized);
        }

        let state = self.normalize_state(state)?;
        let value = self.eval_helper(&state, &bindings, helper, "<helper>")?;
        self.normalize_value_for_type(&value, &helper.returns, TypeValidationMode::SchemaOwned)
            .map_err(|reason| self.eval_error("<helper>", reason))
    }

    fn render_effect(
        &self,
        state: &KernelState,
        bindings: &BTreeMap<String, KernelValue>,
        effect: &EffectEmit,
        transition_name: &str,
    ) -> Result<KernelEffect, TransitionRefusal> {
        let effect_schema = self
            .schema
            .effects
            .variant_named(&effect.variant)
            .map_err(|_| {
                self.eval_error(
                    transition_name,
                    format!("unknown effect variant `{}`", effect.variant),
                )
            })?;
        let mut fields = BTreeMap::new();
        for (name, expr) in &effect.fields {
            let field_schema = effect_schema
                .field_named(name)
                .map_err(|reason| self.eval_error(transition_name, reason.to_string()))?;
            let value = self.eval_expr(state, bindings, expr, transition_name)?;
            let value = self
                .normalize_value_for_type(&value, &field_schema.ty, TypeValidationMode::SchemaOwned)
                .map_err(|reason| {
                    self.eval_error(transition_name, format!("effect field `{name}` {reason}"))
                })?;
            fields.insert(name.clone().into(), value);
        }
        Ok(KernelEffect {
            variant: effect.variant.clone().into(),
            fields,
        })
    }

    fn apply_update(
        &self,
        state: &mut KernelState,
        bindings: &BTreeMap<String, KernelValue>,
        update: &Update,
        transition_name: &str,
    ) -> Result<(), TransitionRefusal> {
        match update {
            Update::Assign { field, expr } => {
                let field_schema = self.state_field_schema(field)?;
                let value = self.eval_expr(state, bindings, expr, transition_name)?;
                let value = self
                    .normalize_value_for_type(
                        &value,
                        &field_schema.ty,
                        TypeValidationMode::SchemaOwned,
                    )
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` {reason}"))
                    })?;
                state.fields.insert(field.clone().into(), value);
            }
            Update::Increment { field, amount } => {
                let current = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::U64(0));
                let value = current
                    .as_u64()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                state.fields.insert(
                    field.clone().into(),
                    KernelValue::U64(value.saturating_add(*amount)),
                );
            }
            Update::Decrement { field, amount } => {
                let current = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::U64(0));
                let value = current
                    .as_u64()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let next = value.checked_sub(*amount).ok_or_else(|| {
                    self.eval_error(transition_name, format!("underflow decrementing `{field}`"))
                })?;
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::U64(next));
            }
            Update::MapInsert { field, key, value } => {
                let (key_ty, value_ty) = self.map_field_types(field)?;
                let key = self.eval_expr(state, bindings, key, transition_name)?;
                let value = self.eval_expr(state, bindings, value, transition_name)?;
                let key = self
                    .normalize_value_for_type(&key, key_ty, TypeValidationMode::SchemaOwned)
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` key {reason}"))
                    })?;
                let value = self
                    .normalize_value_for_type(&value, value_ty, TypeValidationMode::SchemaOwned)
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` value {reason}"))
                    })?;
                let mut map = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::Map(BTreeMap::new()))
                    .into_map()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                map.insert(key, value);
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::Map(map));
            }
            Update::MapRemove { field, key } => {
                let (key_ty, _) = self.map_field_types(field)?;
                let key = self.eval_expr(state, bindings, key, transition_name)?;
                let key = self
                    .normalize_value_for_type(&key, key_ty, TypeValidationMode::SchemaOwned)
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` key {reason}"))
                    })?;
                let mut map = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::Map(BTreeMap::new()))
                    .into_map()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                map.remove(&key);
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::Map(map));
            }
            Update::MapIncrement { field, key, amount } => {
                let (key_ty, _) = self.map_field_types(field)?;
                let key = self.eval_expr(state, bindings, key, transition_name)?;
                let key = self
                    .normalize_value_for_type(&key, key_ty, TypeValidationMode::SchemaOwned)
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` key {reason}"))
                    })?;
                let mut map = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::Map(BTreeMap::new()))
                    .into_map()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let current = map
                    .get(&key)
                    .cloned()
                    .unwrap_or(KernelValue::U64(0))
                    .as_u64()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                map.insert(key, KernelValue::U64(current.saturating_add(*amount)));
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::Map(map));
            }
            Update::MapDecrement { field, key, amount } => {
                let (key_ty, _) = self.map_field_types(field)?;
                let key = self.eval_expr(state, bindings, key, transition_name)?;
                let key = self
                    .normalize_value_for_type(&key, key_ty, TypeValidationMode::SchemaOwned)
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` key {reason}"))
                    })?;
                let mut map = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::Map(BTreeMap::new()))
                    .into_map()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let current = map
                    .get(&key)
                    .cloned()
                    .unwrap_or(KernelValue::U64(0))
                    .as_u64()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let next = current.checked_sub(*amount).ok_or_else(|| {
                    self.eval_error(
                        transition_name,
                        format!("underflow decrementing map entry in `{field}`"),
                    )
                })?;
                map.insert(key, KernelValue::U64(next));
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::Map(map));
            }
            Update::SetInsert { field, value } => {
                let inner_ty = self.set_field_item_type(field)?;
                let value = self.eval_expr(state, bindings, value, transition_name)?;
                let value = self
                    .normalize_value_for_type(&value, inner_ty, TypeValidationMode::SchemaOwned)
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` value {reason}"))
                    })?;
                let mut set = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::Set(BTreeSet::new()))
                    .into_set()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                set.insert(value);
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::Set(set));
            }
            Update::SetRemove { field, value } => {
                let inner_ty = self.set_field_item_type(field)?;
                let value = self.eval_expr(state, bindings, value, transition_name)?;
                let value = self
                    .normalize_value_for_type(&value, inner_ty, TypeValidationMode::SchemaOwned)
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` value {reason}"))
                    })?;
                let mut set = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::Set(BTreeSet::new()))
                    .into_set()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                set.remove(&value);
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::Set(set));
            }
            Update::SeqAppend { field, value } => {
                let inner_ty = self.seq_field_item_type(field)?;
                let value = self.eval_expr(state, bindings, value, transition_name)?;
                let value = self
                    .normalize_value_for_type(&value, inner_ty, TypeValidationMode::SchemaOwned)
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` value {reason}"))
                    })?;
                let mut seq = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::Seq(Vec::new()))
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                seq.push(value);
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::Seq(seq));
            }
            Update::SeqPrepend { field, values } => {
                let inner_ty = self.seq_field_item_type(field)?;
                let values = self.eval_expr(state, bindings, values, transition_name)?;
                let values = self
                    .normalize_value_for_type(
                        &values,
                        &TypeRef::Seq(Box::new(inner_ty.clone())),
                        TypeValidationMode::SchemaOwned,
                    )
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` values {reason}"))
                    })?;
                let mut prefix = values
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let mut seq = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::Seq(Vec::new()))
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                prefix.append(&mut seq);
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::Seq(prefix));
            }
            Update::SeqPopFront { field } => {
                let mut seq = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::Seq(Vec::new()))
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                if !seq.is_empty() {
                    seq.remove(0);
                }
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::Seq(seq));
            }
            Update::SeqRemoveValue { field, value } => {
                let inner_ty = self.seq_field_item_type(field)?;
                let value = self.eval_expr(state, bindings, value, transition_name)?;
                let value = self
                    .normalize_value_for_type(&value, inner_ty, TypeValidationMode::SchemaOwned)
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` value {reason}"))
                    })?;
                let mut seq = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::Seq(Vec::new()))
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                if let Some(index) = seq.iter().position(|item| item == &value) {
                    seq.remove(index);
                }
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::Seq(seq));
            }
            Update::SeqRemoveAll { field, values } => {
                let inner_ty = self.seq_field_item_type(field)?;
                let values = self.eval_expr(state, bindings, values, transition_name)?;
                let values = self
                    .normalize_value_for_type(
                        &values,
                        &TypeRef::Seq(Box::new(inner_ty.clone())),
                        TypeValidationMode::SchemaOwned,
                    )
                    .map_err(|reason| {
                        self.eval_error(transition_name, format!("field `{field}` values {reason}"))
                    })?;
                let values = values
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let mut seq = state
                    .fields
                    .get(field.as_str())
                    .cloned()
                    .unwrap_or(KernelValue::Seq(Vec::new()))
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                for value in values {
                    if let Some(index) = seq.iter().position(|item| item == &value) {
                        seq.remove(index);
                    }
                }
                state
                    .fields
                    .insert(field.clone().into(), KernelValue::Seq(seq));
            }
            Update::Conditional {
                condition,
                then_updates,
                else_updates,
            } => {
                let condition = self.eval_expr(state, bindings, condition, transition_name)?;
                let condition = condition
                    .as_bool()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let branch = if condition {
                    then_updates
                } else {
                    else_updates
                };
                for nested in branch {
                    self.apply_update(state, bindings, nested, transition_name)?;
                }
            }
            Update::ForEach {
                binding,
                over,
                updates,
            } => {
                let values = self.eval_expr(state, bindings, over, transition_name)?;
                let iterable = values
                    .into_iterable()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                for value in iterable {
                    let mut nested = bindings.clone();
                    nested.insert(binding.clone(), value);
                    for nested_update in updates {
                        self.apply_update(state, &nested, nested_update, transition_name)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn eval_expr(
        &self,
        state: &KernelState,
        bindings: &BTreeMap<String, KernelValue>,
        expr: &Expr,
        transition_name: &str,
    ) -> Result<KernelValue, TransitionRefusal> {
        match expr {
            Expr::Bool(value) => Ok(KernelValue::Bool(*value)),
            Expr::U64(value) => Ok(KernelValue::U64(*value)),
            Expr::String(value) => Ok(KernelValue::String(value.clone())),
            Expr::NamedVariant { enum_name, variant } => Ok(KernelValue::NamedVariant {
                enum_name: enum_name.clone().into(),
                variant: variant.clone().into(),
            }),
            Expr::EmptySet => Ok(KernelValue::Set(BTreeSet::new())),
            Expr::EmptyMap => Ok(KernelValue::Map(BTreeMap::new())),
            Expr::SeqLiteral(items) => Ok(KernelValue::Seq(
                items
                    .iter()
                    .map(|item| self.eval_expr(state, bindings, item, transition_name))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Expr::CurrentPhase => Ok(KernelValue::String(state.phase.to_string())),
            Expr::Phase(phase) => Ok(KernelValue::String(phase.clone())),
            Expr::Field(field) => state.fields.get(field.as_str()).cloned().ok_or_else(|| {
                self.eval_error(transition_name, format!("unknown field `{field}`"))
            }),
            Expr::Binding(binding) => bindings.get(binding).cloned().ok_or_else(|| {
                self.eval_error(transition_name, format!("unknown binding `{binding}`"))
            }),
            Expr::Variant(variant) => Ok(KernelValue::String(variant.clone())),
            Expr::None => Ok(KernelValue::None),
            Expr::IfElse {
                condition,
                then_expr,
                else_expr,
            } => {
                let condition = self.eval_expr(state, bindings, condition, transition_name)?;
                let condition = condition
                    .as_bool()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                if condition {
                    self.eval_expr(state, bindings, then_expr, transition_name)
                } else {
                    self.eval_expr(state, bindings, else_expr, transition_name)
                }
            }
            Expr::Not(inner) => {
                let value = self.eval_expr(state, bindings, inner, transition_name)?;
                Ok(KernelValue::Bool(!value.as_bool().map_err(|reason| {
                    self.eval_error(transition_name, reason)
                })?))
            }
            Expr::And(items) => {
                for item in items {
                    let value = self.eval_expr(state, bindings, item, transition_name)?;
                    if !value
                        .as_bool()
                        .map_err(|reason| self.eval_error(transition_name, reason))?
                    {
                        return Ok(KernelValue::Bool(false));
                    }
                }
                Ok(KernelValue::Bool(true))
            }
            Expr::Or(items) => {
                for item in items {
                    let value = self.eval_expr(state, bindings, item, transition_name)?;
                    if value
                        .as_bool()
                        .map_err(|reason| self.eval_error(transition_name, reason))?
                    {
                        return Ok(KernelValue::Bool(true));
                    }
                }
                Ok(KernelValue::Bool(false))
            }
            Expr::Eq(left, right) => Ok(KernelValue::Bool(
                self.eval_expr(state, bindings, left, transition_name)?
                    == self.eval_expr(state, bindings, right, transition_name)?,
            )),
            Expr::Neq(left, right) => Ok(KernelValue::Bool(
                self.eval_expr(state, bindings, left, transition_name)?
                    != self.eval_expr(state, bindings, right, transition_name)?,
            )),
            Expr::Add(left, right) => Ok(KernelValue::U64(
                self.eval_expr(state, bindings, left, transition_name)?
                    .as_u64()
                    .map_err(|reason| self.eval_error(transition_name, reason))?
                    + self
                        .eval_expr(state, bindings, right, transition_name)?
                        .as_u64()
                        .map_err(|reason| self.eval_error(transition_name, reason))?,
            )),
            Expr::Sub(left, right) => {
                let left = self
                    .eval_expr(state, bindings, left, transition_name)?
                    .as_u64()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let right = self
                    .eval_expr(state, bindings, right, transition_name)?
                    .as_u64()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let value = left.checked_sub(right).ok_or_else(|| {
                    self.eval_error(transition_name, "subtraction underflow".to_string())
                })?;
                Ok(KernelValue::U64(value))
            }
            Expr::Gt(left, right) => self.compare_values(
                state,
                bindings,
                left,
                right,
                transition_name,
                |left, right| left > right,
            ),
            Expr::Gte(left, right) => self.compare_values(
                state,
                bindings,
                left,
                right,
                transition_name,
                |left, right| left >= right,
            ),
            Expr::Lt(left, right) => self.compare_values(
                state,
                bindings,
                left,
                right,
                transition_name,
                |left, right| left < right,
            ),
            Expr::Lte(left, right) => self.compare_values(
                state,
                bindings,
                left,
                right,
                transition_name,
                |left, right| left <= right,
            ),
            Expr::Contains { collection, value } => {
                let collection = self.eval_expr(state, bindings, collection, transition_name)?;
                let value = self.eval_expr(state, bindings, value, transition_name)?;
                Ok(KernelValue::Bool(match collection {
                    KernelValue::Seq(items) => items.contains(&value),
                    KernelValue::Set(items) => items.contains(&value),
                    KernelValue::Map(items) => items.contains_key(&value),
                    KernelValue::String(items) => match value {
                        KernelValue::String(needle) => items.contains(&needle),
                        KernelValue::Named { value: needle, .. } => items.contains(&needle),
                        _ => false,
                    },
                    KernelValue::Named { value: items, .. } => match value {
                        KernelValue::String(needle) => items.contains(&needle),
                        KernelValue::Named { value: needle, .. } => items.contains(&needle),
                        _ => false,
                    },
                    KernelValue::NamedVariant { .. }
                    | KernelValue::Bool(_)
                    | KernelValue::U64(_)
                    | KernelValue::None => false,
                }))
            }
            Expr::SeqStartsWith { seq, prefix } => {
                let seq = self.eval_expr(state, bindings, seq, transition_name)?;
                let prefix = self.eval_expr(state, bindings, prefix, transition_name)?;
                let seq = seq
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let prefix = prefix
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                Ok(KernelValue::Bool(seq.starts_with(&prefix)))
            }
            Expr::SeqElements(inner) => {
                let inner = self.eval_expr(state, bindings, inner, transition_name)?;
                let seq = inner
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                Ok(KernelValue::Set(seq.into_iter().collect()))
            }
            Expr::Len(inner) => {
                let inner = self.eval_expr(state, bindings, inner, transition_name)?;
                let len = match inner {
                    KernelValue::Seq(items) => items.len(),
                    KernelValue::Set(items) => items.len(),
                    KernelValue::Map(items) => items.len(),
                    KernelValue::String(items) => items.chars().count(),
                    KernelValue::Named { value, .. } => value.chars().count(),
                    KernelValue::NamedVariant { .. }
                    | KernelValue::Bool(_)
                    | KernelValue::U64(_)
                    | KernelValue::None => {
                        return Err(self.eval_error(
                            transition_name,
                            "Len expects seq, set, map, or string".to_string(),
                        ));
                    }
                };
                Ok(KernelValue::U64(len as u64))
            }
            Expr::Head(inner) => {
                let inner = self.eval_expr(state, bindings, inner, transition_name)?;
                let seq = inner
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                Ok(seq.first().cloned().unwrap_or(KernelValue::None))
            }
            Expr::MapKeys(inner) => {
                let inner = self.eval_expr(state, bindings, inner, transition_name)?;
                let map = inner
                    .into_map()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                Ok(KernelValue::Set(map.keys().cloned().collect()))
            }
            Expr::MapGet { map, key } => {
                let map = self.eval_expr(state, bindings, map, transition_name)?;
                let map = map
                    .into_map()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let key = self.eval_expr(state, bindings, key, transition_name)?;
                Ok(map.get(&key).cloned().unwrap_or(KernelValue::None))
            }
            Expr::MapContainsKey { map, key } => {
                let map = self.eval_expr(state, bindings, map, transition_name)?;
                let map = map
                    .into_map()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let key = self.eval_expr(state, bindings, key, transition_name)?;
                Ok(KernelValue::Bool(map.contains_key(&key)))
            }
            Expr::Some(inner) => Ok(option_some(self.eval_expr(
                state,
                bindings,
                inner,
                transition_name,
            )?)),
            Expr::Call { helper, args } => {
                let helper = self
                    .schema
                    .helpers
                    .iter()
                    .chain(self.schema.derived.iter())
                    .find(|candidate| candidate.name == *helper)
                    .ok_or_else(|| {
                        self.eval_error(transition_name, format!("unknown helper `{helper}`"))
                    })?;

                let mut nested_bindings = BTreeMap::new();
                for (param, arg) in helper.params.iter().zip(args.iter()) {
                    let value = self.eval_expr(state, bindings, arg, transition_name)?;
                    let value = self
                        .normalize_value_for_type(
                            &value,
                            &param.ty,
                            TypeValidationMode::SchemaOwned,
                        )
                        .map_err(|reason| {
                            self.eval_error(
                                transition_name,
                                format!("helper arg `{}` {reason}", param.name),
                            )
                        })?;
                    nested_bindings.insert(param.name.clone(), value);
                }
                let value = self.eval_helper(state, &nested_bindings, helper, transition_name)?;
                self.normalize_value_for_type(
                    &value,
                    &helper.returns,
                    TypeValidationMode::SchemaOwned,
                )
                .map_err(|reason| self.eval_error(transition_name, reason))
            }
            Expr::Quantified {
                quantifier,
                binding,
                over,
                body,
            } => {
                let over = self.eval_expr(state, bindings, over, transition_name)?;
                let iterable = over
                    .into_iterable()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;

                match quantifier {
                    Quantifier::Any => {
                        for value in iterable {
                            let mut nested = bindings.clone();
                            nested.insert(binding.clone(), value);
                            if self
                                .eval_expr(state, &nested, body, transition_name)?
                                .as_bool()
                                .map_err(|reason| self.eval_error(transition_name, reason))?
                            {
                                return Ok(KernelValue::Bool(true));
                            }
                        }
                        Ok(KernelValue::Bool(false))
                    }
                    Quantifier::All => {
                        for value in iterable {
                            let mut nested = bindings.clone();
                            nested.insert(binding.clone(), value);
                            if !self
                                .eval_expr(state, &nested, body, transition_name)?
                                .as_bool()
                                .map_err(|reason| self.eval_error(transition_name, reason))?
                            {
                                return Ok(KernelValue::Bool(false));
                            }
                        }
                        Ok(KernelValue::Bool(true))
                    }
                }
            }
        }
    }

    fn eval_helper(
        &self,
        state: &KernelState,
        bindings: &BTreeMap<String, KernelValue>,
        helper: &HelperSchema,
        transition_name: &str,
    ) -> Result<KernelValue, TransitionRefusal> {
        self.eval_expr(state, bindings, &helper.body, transition_name)
    }

    fn compare_values(
        &self,
        state: &KernelState,
        bindings: &BTreeMap<String, KernelValue>,
        left: &Expr,
        right: &Expr,
        transition_name: &str,
        predicate: impl FnOnce(&KernelValue, &KernelValue) -> bool,
    ) -> Result<KernelValue, TransitionRefusal> {
        let left = self.eval_expr(state, bindings, left, transition_name)?;
        let right = self.eval_expr(state, bindings, right, transition_name)?;
        Ok(KernelValue::Bool(predicate(&left, &right)))
    }

    fn eval_error(&self, transition_name: &str, reason: impl Into<String>) -> TransitionRefusal {
        TransitionRefusal::EvaluationError {
            machine: self.schema.machine.clone(),
            transition: transition_name.to_string(),
            reason: reason.into(),
        }
    }
}

impl KernelValue {
    fn as_bool(&self) -> Result<bool, String> {
        match self {
            Self::Bool(value) => Ok(*value),
            other => Err(format!("expected bool, found {other:?}")),
        }
    }

    fn as_u64(&self) -> Result<u64, String> {
        match self {
            Self::U64(value) => Ok(*value),
            other => Err(format!("expected u64, found {other:?}")),
        }
    }

    pub fn as_string(&self) -> Result<&str, String> {
        match self {
            Self::String(value) | Self::Named { value, .. } => Ok(value.as_str()),
            other => Err(format!("expected string, found {other:?}")),
        }
    }

    pub fn as_named_variant(
        &self,
        expected_enum: impl AsRef<str>,
    ) -> Result<&KernelEnumVariant, String> {
        let expected_enum = expected_enum.as_ref();
        match self {
            Self::NamedVariant { enum_name, variant } if enum_name.as_str() == expected_enum => {
                Ok(variant)
            }
            Self::NamedVariant { enum_name, .. } => Err(format!(
                "expected named variant of enum `{expected_enum}`, found enum `{enum_name}`"
            )),
            other => Err(format!("expected named variant, found {other:?}")),
        }
    }

    #[must_use]
    pub fn is_named_variant(&self, expected: &KernelNamedVariant) -> bool {
        matches!(
            self,
            Self::NamedVariant { enum_name, variant }
                if enum_name == &expected.enum_name && variant == &expected.variant
        )
    }

    fn into_seq(self) -> Result<Vec<KernelValue>, String> {
        match self {
            Self::Seq(items) => Ok(items),
            other => Err(format!("expected seq, found {other:?}")),
        }
    }

    fn into_set(self) -> Result<BTreeSet<KernelValue>, String> {
        match self {
            Self::Set(items) => Ok(items),
            other => Err(format!("expected set, found {other:?}")),
        }
    }

    fn into_map(self) -> Result<BTreeMap<KernelValue, KernelValue>, String> {
        match self {
            Self::Map(items) => Ok(items),
            other => Err(format!("expected map, found {other:?}")),
        }
    }

    fn into_iterable(self) -> Result<Vec<KernelValue>, String> {
        match self {
            Self::Seq(items) => Ok(items),
            Self::Set(items) => Ok(items.into_iter().collect()),
            Self::Map(items) => Ok(items.into_keys().collect()),
            other => Err(format!("expected iterable, found {other:?}")),
        }
    }
}

fn default_value_for_type(ty: &TypeRef) -> KernelValue {
    match ty {
        TypeRef::Bool => KernelValue::Bool(false),
        TypeRef::U32 | TypeRef::U64 => KernelValue::U64(0),
        TypeRef::String => KernelValue::String(String::new()),
        TypeRef::Named(name) if named_type_is_u64(name) => KernelValue::U64(0),
        TypeRef::Named(name) => KernelValue::Named {
            type_name: name.clone().into(),
            value: String::new(),
        },
        TypeRef::Enum(name) => KernelValue::NamedVariant {
            enum_name: name.clone().into(),
            variant: KernelEnumVariant::default(),
        },
        TypeRef::Option(_) => KernelValue::None,
        TypeRef::Set(_) => KernelValue::Set(BTreeSet::new()),
        TypeRef::Seq(_) => KernelValue::Seq(Vec::new()),
        TypeRef::Map(_, _) => KernelValue::Map(BTreeMap::new()),
    }
}

fn named_type_is_u64(name: &str) -> bool {
    matches!(
        name,
        "BoundarySequence" | "TurnNumber" | "FenceToken" | "Generation"
    )
}

fn schema_declares_named_variant(schema: &MachineSchema, enum_name: &str, variant: &str) -> bool {
    schema
        .state
        .init
        .fields
        .iter()
        .any(|init| expr_declares_named_variant(&init.expr, enum_name, variant))
        || schema
            .helpers
            .iter()
            .any(|helper| expr_declares_named_variant(&helper.body, enum_name, variant))
        || schema
            .derived
            .iter()
            .any(|helper| expr_declares_named_variant(&helper.body, enum_name, variant))
        || schema
            .invariants
            .iter()
            .any(|invariant| expr_declares_named_variant(&invariant.expr, enum_name, variant))
        || schema
            .transitions
            .iter()
            .any(|transition| transition_declares_named_variant(transition, enum_name, variant))
}

fn schema_declares_named_variant_family(schema: &MachineSchema, enum_name: &str) -> bool {
    schema
        .state
        .init
        .fields
        .iter()
        .any(|init| expr_declares_named_variant_family(&init.expr, enum_name))
        || schema
            .helpers
            .iter()
            .any(|helper| expr_declares_named_variant_family(&helper.body, enum_name))
        || schema
            .derived
            .iter()
            .any(|helper| expr_declares_named_variant_family(&helper.body, enum_name))
        || schema
            .invariants
            .iter()
            .any(|invariant| expr_declares_named_variant_family(&invariant.expr, enum_name))
        || schema
            .transitions
            .iter()
            .any(|transition| transition_declares_named_variant_family(transition, enum_name))
}

fn transition_declares_named_variant(
    transition: &TransitionSchema,
    enum_name: &str,
    variant: &str,
) -> bool {
    transition
        .guards
        .iter()
        .any(|guard| expr_declares_named_variant(&guard.expr, enum_name, variant))
        || transition
            .updates
            .iter()
            .any(|update| update_declares_named_variant(update, enum_name, variant))
        || transition
            .emit
            .iter()
            .any(|effect| effect_declares_named_variant(effect, enum_name, variant))
}

fn transition_declares_named_variant_family(
    transition: &TransitionSchema,
    enum_name: &str,
) -> bool {
    transition
        .guards
        .iter()
        .any(|guard| expr_declares_named_variant_family(&guard.expr, enum_name))
        || transition
            .updates
            .iter()
            .any(|update| update_declares_named_variant_family(update, enum_name))
        || transition
            .emit
            .iter()
            .any(|effect| effect_declares_named_variant_family(effect, enum_name))
}

fn effect_declares_named_variant(effect: &EffectEmit, enum_name: &str, variant: &str) -> bool {
    effect
        .fields
        .values()
        .any(|expr| expr_declares_named_variant(expr, enum_name, variant))
}

fn effect_declares_named_variant_family(effect: &EffectEmit, enum_name: &str) -> bool {
    effect
        .fields
        .values()
        .any(|expr| expr_declares_named_variant_family(expr, enum_name))
}

fn update_declares_named_variant(update: &Update, enum_name: &str, variant: &str) -> bool {
    match update {
        Update::Assign { expr, .. }
        | Update::SetInsert { value: expr, .. }
        | Update::SetRemove { value: expr, .. }
        | Update::SeqAppend { value: expr, .. }
        | Update::SeqPrepend { values: expr, .. }
        | Update::SeqRemoveValue { value: expr, .. }
        | Update::SeqRemoveAll { values: expr, .. } => {
            expr_declares_named_variant(expr, enum_name, variant)
        }
        Update::MapInsert { key, value, .. } => {
            expr_declares_named_variant(key, enum_name, variant)
                || expr_declares_named_variant(value, enum_name, variant)
        }
        Update::MapIncrement { key, .. }
        | Update::MapDecrement { key, .. }
        | Update::MapRemove { key, .. } => expr_declares_named_variant(key, enum_name, variant),
        Update::Increment { .. } | Update::Decrement { .. } | Update::SeqPopFront { .. } => false,
        Update::Conditional {
            condition,
            then_updates,
            else_updates,
        } => {
            expr_declares_named_variant(condition, enum_name, variant)
                || then_updates
                    .iter()
                    .any(|update| update_declares_named_variant(update, enum_name, variant))
                || else_updates
                    .iter()
                    .any(|update| update_declares_named_variant(update, enum_name, variant))
        }
        Update::ForEach { over, updates, .. } => {
            expr_declares_named_variant(over, enum_name, variant)
                || updates
                    .iter()
                    .any(|update| update_declares_named_variant(update, enum_name, variant))
        }
    }
}

fn update_declares_named_variant_family(update: &Update, enum_name: &str) -> bool {
    match update {
        Update::Assign { expr, .. }
        | Update::SetInsert { value: expr, .. }
        | Update::SetRemove { value: expr, .. }
        | Update::SeqAppend { value: expr, .. }
        | Update::SeqPrepend { values: expr, .. }
        | Update::SeqRemoveValue { value: expr, .. }
        | Update::SeqRemoveAll { values: expr, .. } => {
            expr_declares_named_variant_family(expr, enum_name)
        }
        Update::MapInsert { key, value, .. } => {
            expr_declares_named_variant_family(key, enum_name)
                || expr_declares_named_variant_family(value, enum_name)
        }
        Update::MapIncrement { key, .. }
        | Update::MapDecrement { key, .. }
        | Update::MapRemove { key, .. } => expr_declares_named_variant_family(key, enum_name),
        Update::Increment { .. } | Update::Decrement { .. } | Update::SeqPopFront { .. } => false,
        Update::Conditional {
            condition,
            then_updates,
            else_updates,
        } => {
            expr_declares_named_variant_family(condition, enum_name)
                || then_updates
                    .iter()
                    .any(|update| update_declares_named_variant_family(update, enum_name))
                || else_updates
                    .iter()
                    .any(|update| update_declares_named_variant_family(update, enum_name))
        }
        Update::ForEach { over, updates, .. } => {
            expr_declares_named_variant_family(over, enum_name)
                || updates
                    .iter()
                    .any(|update| update_declares_named_variant_family(update, enum_name))
        }
    }
}

fn expr_declares_named_variant(expr: &Expr, enum_name: &str, variant: &str) -> bool {
    match expr {
        Expr::NamedVariant {
            enum_name: expr_enum,
            variant: expr_variant,
        } => expr_enum == enum_name && expr_variant == variant,
        Expr::SeqLiteral(items) | Expr::And(items) | Expr::Or(items) => items
            .iter()
            .any(|item| expr_declares_named_variant(item, enum_name, variant)),
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            expr_declares_named_variant(condition, enum_name, variant)
                || expr_declares_named_variant(then_expr, enum_name, variant)
                || expr_declares_named_variant(else_expr, enum_name, variant)
        }
        Expr::Not(inner)
        | Expr::SeqElements(inner)
        | Expr::Len(inner)
        | Expr::Head(inner)
        | Expr::MapKeys(inner)
        | Expr::Some(inner) => expr_declares_named_variant(inner, enum_name, variant),
        Expr::Eq(left, right)
        | Expr::Neq(left, right)
        | Expr::Add(left, right)
        | Expr::Sub(left, right)
        | Expr::Gt(left, right)
        | Expr::Gte(left, right)
        | Expr::Lt(left, right)
        | Expr::Lte(left, right) => {
            expr_declares_named_variant(left, enum_name, variant)
                || expr_declares_named_variant(right, enum_name, variant)
        }
        Expr::Contains { collection, value } => {
            expr_declares_named_variant(collection, enum_name, variant)
                || expr_declares_named_variant(value, enum_name, variant)
        }
        Expr::MapContainsKey { map, key } | Expr::MapGet { map, key } => {
            expr_declares_named_variant(map, enum_name, variant)
                || expr_declares_named_variant(key, enum_name, variant)
        }
        Expr::SeqStartsWith { seq, prefix } => {
            expr_declares_named_variant(seq, enum_name, variant)
                || expr_declares_named_variant(prefix, enum_name, variant)
        }
        Expr::Call { args, .. } => args
            .iter()
            .any(|arg| expr_declares_named_variant(arg, enum_name, variant)),
        Expr::Quantified { over, body, .. } => {
            expr_declares_named_variant(over, enum_name, variant)
                || expr_declares_named_variant(body, enum_name, variant)
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
        | Expr::None => false,
    }
}

fn expr_declares_named_variant_family(expr: &Expr, enum_name: &str) -> bool {
    match expr {
        Expr::NamedVariant {
            enum_name: expr_enum,
            ..
        } => expr_enum == enum_name,
        Expr::SeqLiteral(items) | Expr::And(items) | Expr::Or(items) => items
            .iter()
            .any(|item| expr_declares_named_variant_family(item, enum_name)),
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            expr_declares_named_variant_family(condition, enum_name)
                || expr_declares_named_variant_family(then_expr, enum_name)
                || expr_declares_named_variant_family(else_expr, enum_name)
        }
        Expr::Not(inner)
        | Expr::SeqElements(inner)
        | Expr::Len(inner)
        | Expr::Head(inner)
        | Expr::MapKeys(inner)
        | Expr::Some(inner) => expr_declares_named_variant_family(inner, enum_name),
        Expr::Eq(left, right)
        | Expr::Neq(left, right)
        | Expr::Add(left, right)
        | Expr::Sub(left, right)
        | Expr::Gt(left, right)
        | Expr::Gte(left, right)
        | Expr::Lt(left, right)
        | Expr::Lte(left, right) => {
            expr_declares_named_variant_family(left, enum_name)
                || expr_declares_named_variant_family(right, enum_name)
        }
        Expr::Contains { collection, value } => {
            expr_declares_named_variant_family(collection, enum_name)
                || expr_declares_named_variant_family(value, enum_name)
        }
        Expr::MapContainsKey { map, key } | Expr::MapGet { map, key } => {
            expr_declares_named_variant_family(map, enum_name)
                || expr_declares_named_variant_family(key, enum_name)
        }
        Expr::SeqStartsWith { seq, prefix } => {
            expr_declares_named_variant_family(seq, enum_name)
                || expr_declares_named_variant_family(prefix, enum_name)
        }
        Expr::Call { args, .. } => args
            .iter()
            .any(|arg| expr_declares_named_variant_family(arg, enum_name)),
        Expr::Quantified { over, body, .. } => {
            expr_declares_named_variant_family(over, enum_name)
                || expr_declares_named_variant_family(body, enum_name)
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
        | Expr::None => false,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use meerkat_machine_schema::canonical_machine_schemas;
    use meerkat_machine_schema::catalog::dsl::{
        dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
    };
    use meerkat_machine_schema::runtime_local::flow_frame_machine;

    use super::{
        GeneratedMachineKernel, KernelField, KernelInput, KernelNamedVariant, KernelSignal,
        KernelState, KernelValue, TransitionRefusal,
    };

    fn named(type_name: &'static str, value: &str) -> KernelValue {
        KernelValue::Named {
            type_name: type_name.into(),
            value: value.to_owned(),
        }
    }

    fn named_variant(enum_name: &'static str, variant: &'static str) -> KernelValue {
        KernelValue::NamedVariant {
            enum_name: enum_name.into(),
            variant: variant.into(),
        }
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn every_catalog_machine_builds_an_initial_state() {
        for schema in canonical_machine_schemas() {
            let kernel = GeneratedMachineKernel::new(schema.clone());
            let state = kernel.initial_state().expect("initial state");
            assert_eq!(state.phase, schema.state.init.phase);
        }
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn meerkat_rejects_unknown_input_variant() {
        let kernel = GeneratedMachineKernel::new(meerkat_machine());
        let state = kernel.initial_state().expect("initial state");
        let refusal = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "DoesNotExist".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect_err("unknown input variant should fail");
        assert!(matches!(
            refusal,
            TransitionRefusal::UnknownInputVariant { .. }
        ));
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn meerkat_prepare_bindings_rejects_bad_payload_types() {
        let kernel = GeneratedMachineKernel::new(meerkat_machine());
        let state = kernel.initial_state().expect("initial state");
        let initialized = kernel
            .transition_signal(
                &state,
                &KernelSignal {
                    variant: "Initialize".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("initialize");
        let registered = kernel
            .transition(
                &initialized.next_state,
                &KernelInput {
                    variant: "RegisterSession".into(),
                    fields: BTreeMap::from([(
                        "session_id".into(),
                        named("SessionId", "session-1"),
                    )]),
                },
            )
            .expect("register session");
        let refusal = kernel
            .transition(
                &registered.next_state,
                &KernelInput {
                    variant: "PrepareBindings".into(),
                    fields: BTreeMap::from([
                        ("agent_runtime_id".into(), KernelValue::U64(7)),
                        ("fence_token".into(), KernelValue::U64(3)),
                        ("generation".into(), KernelValue::U64(1)),
                    ]),
                },
            )
            .expect_err("invalid runtime id payload should fail");
        assert!(matches!(
            refusal,
            TransitionRefusal::InvalidInputPayload { .. }
        ));
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn mob_spawn_and_submit_work_transitions_execute() {
        let kernel = GeneratedMachineKernel::new(mob_machine());
        let state = kernel.initial_state().expect("initial state");
        assert_eq!(state.phase, "Running");
        let running = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "Spawn".into(),
                    fields: BTreeMap::from([
                        (
                            "agent_identity".into(),
                            named("AgentIdentity", "agent.worker"),
                        ),
                        (
                            "agent_runtime_id".into(),
                            named("AgentRuntimeId", "runtime.worker.1"),
                        ),
                        ("fence_token".into(), KernelValue::U64(41)),
                        ("generation".into(), KernelValue::U64(2)),
                        ("external_addressable".into(), KernelValue::Bool(false)),
                        // W3-H-1: new realtime-binding fields. `replacing`
                        // is None (no prior binding) so the Fresh branch
                        // fires.
                        (
                            "bridge_session_id".into(),
                            named("SessionId", "bridge.worker.1"),
                        ),
                        ("replacing".into(), KernelValue::None),
                    ]),
                },
            )
            .expect("spawn member");
        assert_eq!(running.transition, "SpawnRunningFresh");
        assert_eq!(running.next_state.phase, "Running");
        assert!(
            running
                .effects
                .iter()
                .any(|effect| effect.variant == "RequestRuntimeBinding")
        );
        let submitted = kernel
            .transition(
                &running.next_state,
                &KernelInput {
                    variant: "SubmitWork".into(),
                    fields: BTreeMap::from([
                        (
                            "agent_runtime_id".into(),
                            named("AgentRuntimeId", "runtime.worker.1"),
                        ),
                        ("fence_token".into(), KernelValue::U64(41)),
                        ("work_id".into(), named("WorkId", "work-1")),
                        ("origin".into(), named_variant("WorkOrigin", "Internal")),
                    ]),
                },
            )
            .expect("submit work");
        assert_eq!(submitted.transition, "SubmitWorkRunningInternal");
        // SubmitMemberWork effect was removed (unimplemented route to MeerkatMachine).
        assert!(
            !submitted
                .effects
                .iter()
                .any(|effect| effect.variant == "SubmitMemberWork")
        );
        assert_eq!(
            submitted.next_state.fields.get("active_run_count"),
            Some(&KernelValue::U64(0))
        );
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn mob_observe_runtime_destroyed_clears_startup_and_kickoff_sets() {
        let kernel = GeneratedMachineKernel::new(mob_machine());
        let state = kernel.initial_state().expect("initial state");
        let runtime_id = "runtime.lead.1";
        let member_id = "lead-destroy";

        let spawned = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "Spawn".into(),
                    fields: BTreeMap::from([
                        (
                            "agent_identity".into(),
                            named("AgentIdentity", "agent.lead"),
                        ),
                        (
                            "agent_runtime_id".into(),
                            named("AgentRuntimeId", runtime_id),
                        ),
                        ("fence_token".into(), KernelValue::U64(41)),
                        ("generation".into(), KernelValue::U64(2)),
                        ("external_addressable".into(), KernelValue::Bool(false)),
                        (
                            "bridge_session_id".into(),
                            named("SessionId", "bridge.lead.1"),
                        ),
                        ("replacing".into(), KernelValue::None),
                    ]),
                },
            )
            .expect("spawn lead");
        let runtime_ready = kernel
            .transition_signal(
                &spawned.next_state,
                &KernelSignal {
                    variant: "ObserveRuntimeReady".into(),
                    fields: BTreeMap::from([
                        (
                            "agent_runtime_id".into(),
                            named("AgentRuntimeId", runtime_id),
                        ),
                        ("fence_token".into(), KernelValue::U64(41)),
                    ]),
                },
            )
            .expect("observe runtime ready");
        let startup_ready = kernel
            .transition(
                &runtime_ready.next_state,
                &KernelInput {
                    variant: "StartupMarkReady".into(),
                    fields: BTreeMap::from([
                        (
                            "agent_runtime_id".into(),
                            named("AgentRuntimeId", runtime_id),
                        ),
                        ("fence_token".into(), KernelValue::U64(41)),
                    ]),
                },
            )
            .expect("mark startup ready");
        let kickoff_pending = kernel
            .transition(
                &startup_ready.next_state,
                &KernelInput {
                    variant: "KickoffMarkPending".into(),
                    fields: BTreeMap::from([(
                        "member_id".into(),
                        KernelValue::String(member_id.into()),
                    )]),
                },
            )
            .expect("mark kickoff pending");
        let kickoff_starting = kernel
            .transition(
                &kickoff_pending.next_state,
                &KernelInput {
                    variant: "KickoffMarkStarting".into(),
                    fields: BTreeMap::from([(
                        "member_id".into(),
                        KernelValue::String(member_id.into()),
                    )]),
                },
            )
            .expect("mark kickoff starting");

        assert_eq!(
            kickoff_starting
                .next_state
                .fields
                .get("member_startup_ready"),
            Some(&KernelValue::Set(BTreeSet::from([named(
                "AgentRuntimeId",
                runtime_id,
            )]))),
            "setup must prove the startup-ready set is populated before destroy"
        );
        assert_eq!(
            kickoff_starting
                .next_state
                .fields
                .get("identity_to_runtime"),
            Some(&KernelValue::Map(BTreeMap::from([(
                named("AgentIdentity", "agent.lead"),
                named("AgentRuntimeId", runtime_id),
            )]))),
            "spawn must populate identity_to_runtime before destroy"
        );
        assert_eq!(
            kickoff_starting
                .next_state
                .fields
                .get("member_realtime_bindings"),
            Some(&KernelValue::Map(BTreeMap::from([(
                named("AgentIdentity", "agent.lead"),
                named("SessionId", "bridge.lead.1"),
            )]))),
            "spawn must populate member_realtime_bindings before destroy"
        );
        assert_eq!(
            kickoff_starting
                .next_state
                .fields
                .get("member_kickoff_starting"),
            Some(&KernelValue::Set(BTreeSet::from([KernelValue::String(
                member_id.into(),
            )]))),
            "setup must prove the kickoff state is populated before destroy"
        );

        let destroyed = kernel
            .transition_signal(
                &kickoff_starting.next_state,
                &KernelSignal {
                    variant: "ObserveRuntimeDestroyed".into(),
                    fields: BTreeMap::from([
                        (
                            "agent_runtime_id".into(),
                            named("AgentRuntimeId", runtime_id),
                        ),
                        ("fence_token".into(), KernelValue::U64(41)),
                    ]),
                },
            )
            .expect("observe runtime destroyed");

        for field in [
            "member_startup_binding_requested",
            "member_startup_runtime_ready",
            "member_startup_ready",
            "member_kickoff_pending",
            "member_kickoff_starting",
            "member_kickoff_started",
            "member_kickoff_failed",
            "member_kickoff_cancelled",
        ] {
            assert_eq!(
                destroyed.next_state.fields.get(field),
                Some(&KernelValue::Set(BTreeSet::new())),
                "{field} should be cleared when the runtime is destroyed"
            );
        }
        assert_eq!(
            destroyed.next_state.fields.get("member_kickoff_error"),
            Some(&KernelValue::Map(BTreeMap::new())),
            "destroy should clear kickoff errors alongside the startup/kickoff sets"
        );
        assert_eq!(
            destroyed.next_state.fields.get("identity_to_runtime"),
            Some(&KernelValue::Map(BTreeMap::new())),
            "destroy should clear identity_to_runtime alongside runtime teardown"
        );
        assert_eq!(
            destroyed.next_state.fields.get("member_realtime_bindings"),
            Some(&KernelValue::Map(BTreeMap::new())),
            "destroy should clear member_realtime_bindings alongside runtime teardown"
        );
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn mob_kickoff_clear_preserves_stopped_phase() {
        let kernel = GeneratedMachineKernel::new(mob_machine());
        let state = kernel.initial_state().expect("initial state");
        let stopped = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "Stop".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("stop");
        assert_eq!(stopped.next_state.phase, "Stopped");

        let pending = kernel
            .transition(
                &stopped.next_state,
                &KernelInput {
                    variant: "KickoffMarkPending".into(),
                    fields: BTreeMap::from([(
                        "member_id".into(),
                        KernelValue::String("lead-stopped".into()),
                    )]),
                },
            )
            .expect("mark kickoff pending while stopped");
        assert_eq!(
            pending.next_state.phase, "Stopped",
            "kickoff bookkeeping must not revive a stopped mob"
        );

        let cleared = kernel
            .transition(
                &pending.next_state,
                &KernelInput {
                    variant: "KickoffClear".into(),
                    fields: BTreeMap::from([(
                        "member_id".into(),
                        KernelValue::String("lead-stopped".into()),
                    )]),
                },
            )
            .expect("clear kickoff while stopped");
        assert_eq!(
            cleared.next_state.phase, "Stopped",
            "kickoff clear must preserve the stopped lifecycle phase"
        );
        assert_eq!(
            cleared.next_state.fields.get("member_kickoff_pending"),
            Some(&KernelValue::Set(BTreeSet::new())),
            "kickoff clear should still remove the per-member pending marker"
        );
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn mob_destroy_clears_identity_and_realtime_binding_state() {
        let kernel = GeneratedMachineKernel::new(mob_machine());
        let state = kernel.initial_state().expect("initial state");
        let spawned = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "Spawn".into(),
                    fields: BTreeMap::from([
                        (
                            "agent_identity".into(),
                            named("AgentIdentity", "agent.destroy"),
                        ),
                        (
                            "agent_runtime_id".into(),
                            named("AgentRuntimeId", "runtime.destroy.1"),
                        ),
                        ("fence_token".into(), KernelValue::U64(7)),
                        ("generation".into(), KernelValue::U64(1)),
                        ("external_addressable".into(), KernelValue::Bool(true)),
                        (
                            "bridge_session_id".into(),
                            named("SessionId", "bridge.destroy.1"),
                        ),
                        ("replacing".into(), KernelValue::None),
                    ]),
                },
            )
            .expect("spawn member");
        let destroyed = kernel
            .transition(
                &spawned.next_state,
                &KernelInput {
                    variant: "Destroy".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("destroy");
        assert_eq!(destroyed.next_state.phase, "Destroyed");
        assert_eq!(
            destroyed
                .next_state
                .fields
                .get("externally_addressable_runtime_ids"),
            Some(&KernelValue::Set(BTreeSet::new())),
            "destroy should clear externally addressable runtime ids"
        );
        assert_eq!(
            destroyed.next_state.fields.get("identity_to_runtime"),
            Some(&KernelValue::Map(BTreeMap::new())),
            "destroy should clear identity_to_runtime"
        );
        assert_eq!(
            destroyed.next_state.fields.get("member_realtime_bindings"),
            Some(&KernelValue::Map(BTreeMap::new())),
            "destroy should clear member_realtime_bindings"
        );
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn kernel_value_map_roundtrips_through_json() {
        let value = KernelValue::Map(BTreeMap::from([
            (
                KernelValue::String("members".into()),
                KernelValue::Set(BTreeSet::from([
                    KernelValue::String("agent.a".into()),
                    KernelValue::String("agent.b".into()),
                ])),
            ),
            (KernelValue::String("count".into()), KernelValue::U64(2)),
        ]));

        let encoded = serde_json::to_string(&value).expect("serialize kernel value");
        let decoded: KernelValue =
            serde_json::from_str(&encoded).expect("deserialize kernel value");

        assert_eq!(decoded, value);
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn kernel_value_named_roundtrips_through_json() {
        let value = named("FrameId", "frame-typed-1");

        let encoded = serde_json::to_string(&value).expect("serialize typed named kernel value");
        let decoded: KernelValue =
            serde_json::from_str(&encoded).expect("deserialize typed named kernel value");

        assert_eq!(decoded, value);
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_frame_initial_state_uses_typed_named_defaults() {
        let kernel = GeneratedMachineKernel::new(flow_frame_machine());
        let state = kernel.initial_state().expect("initial state");

        assert_eq!(
            state.field(&KernelField::from("frame_id")),
            Some(&named("FrameId", ""))
        );
        assert_eq!(
            state.field(&KernelField::from("loop_instance_id")),
            Some(&named("LoopInstanceId", ""))
        );
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_frame_coerces_plain_string_named_id_payload_at_boundary() {
        let kernel = GeneratedMachineKernel::new(flow_frame_machine());
        let state = kernel.initial_state().expect("initial state");

        let outcome = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "StartRootFrame".into(),
                    fields: BTreeMap::from([
                        ("frame_id".into(), KernelValue::String("frame-1".into())),
                        ("tracked_nodes".into(), KernelValue::Set(BTreeSet::new())),
                        ("ordered_nodes".into(), KernelValue::Seq(Vec::new())),
                        ("node_kind".into(), KernelValue::Map(BTreeMap::new())),
                        (
                            "node_dependencies".into(),
                            KernelValue::Map(BTreeMap::new()),
                        ),
                        (
                            "node_dependency_modes".into(),
                            KernelValue::Map(BTreeMap::new()),
                        ),
                        ("node_branches".into(), KernelValue::Map(BTreeMap::new())),
                    ]),
                },
            )
            .expect("plain string frame_id should be normalized at the public boundary");

        assert_eq!(
            outcome.next_state.field(&KernelField::from("frame_id")),
            Some(&named("FrameId", "frame-1"))
        );
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_frame_rejects_invalid_declared_enum_variant() {
        let kernel = GeneratedMachineKernel::new(flow_frame_machine());
        let state = kernel.initial_state().expect("initial state");

        let refusal = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "StartRootFrame".into(),
                    fields: BTreeMap::from([
                        ("frame_id".into(), named("FrameId", "frame-1")),
                        (
                            "tracked_nodes".into(),
                            KernelValue::Set(BTreeSet::from([named("FlowNodeId", "node-a")])),
                        ),
                        (
                            "ordered_nodes".into(),
                            KernelValue::Seq(vec![named("FlowNodeId", "node-a")]),
                        ),
                        (
                            "node_kind".into(),
                            KernelValue::Map(BTreeMap::from([(
                                named("FlowNodeId", "node-a"),
                                named_variant("FlowNodeKind", "Typo"),
                            )])),
                        ),
                        (
                            "node_dependencies".into(),
                            KernelValue::Map(BTreeMap::new()),
                        ),
                        (
                            "node_dependency_modes".into(),
                            KernelValue::Map(BTreeMap::new()),
                        ),
                        ("node_branches".into(), KernelValue::Map(BTreeMap::new())),
                    ]),
                },
            )
            .expect_err("undeclared enum variant should fail typed kernel validation");

        assert!(matches!(
            refusal,
            TransitionRefusal::InvalidInputPayload { .. }
        ));
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_frame_accepts_typed_named_ids_and_declared_enum_variants() {
        let kernel = GeneratedMachineKernel::new(flow_frame_machine());
        let state = kernel.initial_state().expect("initial state");

        let outcome = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "StartRootFrame".into(),
                    fields: BTreeMap::from([
                        ("frame_id".into(), named("FrameId", "frame-1")),
                        ("tracked_nodes".into(), KernelValue::Set(BTreeSet::new())),
                        ("ordered_nodes".into(), KernelValue::Seq(Vec::new())),
                        ("node_kind".into(), KernelValue::Map(BTreeMap::new())),
                        (
                            "node_dependencies".into(),
                            KernelValue::Map(BTreeMap::new()),
                        ),
                        (
                            "node_dependency_modes".into(),
                            KernelValue::Map(BTreeMap::new()),
                        ),
                        ("node_branches".into(), KernelValue::Map(BTreeMap::new())),
                    ]),
                },
            )
            .expect("typed frame id should be accepted");

        assert_eq!(
            outcome.next_state.field(&KernelField::from("frame_id")),
            Some(&named("FrameId", "frame-1"))
        );
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn kernel_state_deserializes_legacy_stringly_json() {
        let encoded = r#"{
            "phase": "Running",
            "fields": {
                "frame_id": { "type": "string", "value": "frame-1" },
                "frame_scope": {
                    "type": "named_variant",
                    "enum_name": "FrameScope",
                    "variant": "Body"
                }
            }
        }"#;

        let decoded: KernelState =
            serde_json::from_str(encoded).expect("deserialize legacy kernel state");

        assert_eq!(decoded.phase, "Running");
        assert_eq!(
            decoded.fields.get("frame_id"),
            Some(&KernelValue::String("frame-1".into()))
        );
        assert!(
            decoded
                .field(&KernelField::from("frame_scope"))
                .is_some_and(|value| value
                    .is_named_variant(&KernelNamedVariant::new_static("FrameScope", "Body",)))
        );
    }
}
