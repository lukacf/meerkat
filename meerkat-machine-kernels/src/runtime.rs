use std::collections::{BTreeMap, BTreeSet};

use meerkat_machine_schema::identity::{
    EffectVariantId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, MachineId, PhaseId,
    SignalVariantId, TransitionId,
};
use meerkat_machine_schema::{
    EffectEmit, Expr, HelperSchema, MachineSchema, Quantifier, RouteVariantId, TransitionSchema,
    TriggerMatch, TypeRef, Update,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum KernelValue {
    Bool(bool),
    U64(u64),
    String(String),
    NamedVariant {
        enum_name: EnumTypeId,
        variant: EnumVariantId,
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
    NamedVariant {
        enum_name: EnumTypeId,
        variant: EnumVariantId,
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

fn option_map_matches_inner(value: &KernelValue, inner_ty: &TypeRef) -> bool {
    let KernelValue::Map(entries) = value else {
        return false;
    };
    if entries.len() != 1 {
        return false;
    }
    let Some(inner) = entries.get(&KernelValue::String("value".to_string())) else {
        return false;
    };
    value_matches_type(inner, inner_ty)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelState {
    pub phase: PhaseId,
    pub fields: BTreeMap<FieldId, KernelValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelInput {
    pub variant: InputVariantId,
    pub fields: BTreeMap<FieldId, KernelValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelSignal {
    pub variant: SignalVariantId,
    pub fields: BTreeMap<FieldId, KernelValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelEffect {
    pub variant: EffectVariantId,
    pub fields: BTreeMap<FieldId, KernelValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionOutcome {
    pub transition: TransitionId,
    pub next_state: KernelState,
    pub effects: Vec<KernelEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransitionRefusal {
    #[error("unknown input variant `{variant}` for machine `{machine}`")]
    UnknownInputVariant {
        machine: MachineId,
        variant: InputVariantId,
    },
    #[error("unknown signal variant `{variant}` for machine `{machine}`")]
    UnknownSignalVariant {
        machine: MachineId,
        variant: SignalVariantId,
    },
    #[error("invalid input payload for machine `{machine}` variant `{variant}`: {reason}")]
    InvalidInputPayload {
        machine: MachineId,
        variant: InputVariantId,
        reason: String,
    },
    #[error("invalid signal payload for machine `{machine}` variant `{variant}`: {reason}")]
    InvalidSignalPayload {
        machine: MachineId,
        variant: SignalVariantId,
        reason: String,
    },
    #[error(
        "no matching transition for machine `{machine}` in phase `{phase}` on `{}`",
        .trigger.as_str()
    )]
    NoMatchingTransition {
        machine: MachineId,
        phase: PhaseId,
        trigger: RouteVariantId,
    },
    #[error(
        "ambiguous transitions for machine `{machine}` in phase `{phase}` on `{}`: {transitions:?}",
        .trigger.as_str()
    )]
    AmbiguousTransition {
        machine: MachineId,
        phase: PhaseId,
        trigger: RouteVariantId,
        transitions: Vec<TransitionId>,
    },
    #[error("evaluation error in machine `{machine}` transition `{transition}`: {reason}")]
    EvaluationError {
        machine: MachineId,
        transition: TransitionId,
        reason: String,
    },
}

#[derive(Debug, Clone)]
pub struct GeneratedMachineKernel {
    schema: MachineSchema,
}

#[allow(dead_code)]
pub(crate) type RawValue = KernelValue;
#[allow(dead_code)]
pub(crate) type RawState = KernelState;
#[allow(dead_code)]
pub(crate) type RawInput = KernelInput;
#[allow(dead_code)]
pub(crate) type RawSignal = KernelSignal;
#[allow(dead_code)]
pub(crate) type RawEffect = KernelEffect;
#[allow(dead_code)]
pub(crate) type RawOutcome = TransitionOutcome;
#[allow(dead_code)]
pub(crate) type RawRefusal = TransitionRefusal;

impl GeneratedMachineKernel {
    #[must_use]
    pub fn new(schema: MachineSchema) -> Self {
        Self { schema }
    }

    pub fn initial_state(&self) -> Result<KernelState, TransitionRefusal> {
        let helper_transition = helper_transition_id();
        let mut state = KernelState {
            phase: self.schema.state.init.phase.clone(),
            fields: self
                .schema
                .state
                .fields
                .iter()
                .map(|field| (field.name.clone(), default_value_for_type(&field.ty)))
                .collect(),
        };

        for init in &self.schema.state.init.fields {
            let value = self.eval_expr(&state, &BTreeMap::new(), &init.expr, &helper_transition)?;
            state.fields.insert(init.field.clone(), value);
        }

        Ok(state)
    }

    pub fn transition(
        &self,
        state: &KernelState,
        input: &KernelInput,
    ) -> Result<TransitionOutcome, TransitionRefusal> {
        let trigger = RouteVariantId::Input(input.variant.clone());
        self.transition_with_trigger(state, &trigger, &input.fields)
    }

    pub fn transition_signal(
        &self,
        state: &KernelState,
        signal: &KernelSignal,
    ) -> Result<TransitionOutcome, TransitionRefusal> {
        let trigger = RouteVariantId::Signal(signal.variant.clone());
        self.transition_with_trigger(state, &trigger, &signal.fields)
    }

    fn transition_with_trigger(
        &self,
        state: &KernelState,
        trigger: &RouteVariantId,
        fields: &BTreeMap<FieldId, KernelValue>,
    ) -> Result<TransitionOutcome, TransitionRefusal> {
        let trigger_variant = match trigger {
            RouteVariantId::Input(variant) => self
                .schema
                .inputs
                .variant_named(variant.as_str())
                .map_err(|_| TransitionRefusal::UnknownInputVariant {
                    machine: self.schema.machine.clone(),
                    variant: variant.clone(),
                })?,
            RouteVariantId::Signal(variant) => self
                .schema
                .signals
                .variant_named(variant.as_str())
                .map_err(|_| TransitionRefusal::UnknownSignalVariant {
                    machine: self.schema.machine.clone(),
                    variant: variant.clone(),
                })?,
        };

        for field in &trigger_variant.fields {
            let Some(value) = fields.get(&field.name) else {
                return Err(
                    self.payload_refusal(trigger, format!("missing field `{}`", field.name))
                );
            };
            if !value_matches_type(value, &field.ty) {
                return Err(self.payload_refusal(
                    trigger,
                    format!("field `{}` does not match declared type", field.name),
                ));
            }
        }

        let mut matches = Vec::new();
        for transition in &self.schema.transitions {
            if !transition.from.iter().any(|phase| phase == &state.phase) {
                continue;
            }
            if !trigger_match_matches(&transition.on, trigger) {
                continue;
            }

            let mut bindings = BTreeMap::new();
            let mut malformed = false;
            for binding in transition.on.bindings() {
                let Some(value) = fields.get(binding) else {
                    malformed = true;
                    break;
                };
                bindings.insert(binding.as_str().to_owned(), value.clone());
            }
            if malformed {
                return Err(self.payload_refusal(
                    trigger,
                    "transition binding missing from payload".to_string(),
                ));
            }

            let guards_hold = transition.guards.iter().try_fold(true, |acc, guard| {
                let value = self.eval_expr(state, &bindings, &guard.expr, &transition.name)?;
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
                phase: state.phase.clone(),
                trigger: trigger.clone(),
            }),
            1 => {
                let Some((transition, bindings)) = matches.pop() else {
                    return Err(TransitionRefusal::NoMatchingTransition {
                        machine: self.schema.machine.clone(),
                        phase: state.phase.clone(),
                        trigger: trigger.clone(),
                    });
                };
                self.apply_transition(state, transition, &bindings)
            }
            _ => Err(TransitionRefusal::AmbiguousTransition {
                machine: self.schema.machine.clone(),
                phase: state.phase.clone(),
                trigger: trigger.clone(),
                transitions: matches
                    .iter()
                    .map(|(transition, _)| transition.name.clone())
                    .collect(),
            }),
        }
    }

    fn payload_refusal(&self, trigger: &RouteVariantId, reason: String) -> TransitionRefusal {
        match trigger {
            RouteVariantId::Input(variant) => TransitionRefusal::InvalidInputPayload {
                machine: self.schema.machine.clone(),
                variant: variant.clone(),
                reason,
            },
            RouteVariantId::Signal(variant) => TransitionRefusal::InvalidSignalPayload {
                machine: self.schema.machine.clone(),
                variant: variant.clone(),
                reason,
            },
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
        next_state.phase = transition.to.clone();

        let mut effects = Vec::new();
        for effect in &transition.emit {
            effects.push(self.render_effect(&next_state, bindings, effect, &transition.name)?);
        }

        Ok(TransitionOutcome {
            transition: transition.name.clone(),
            next_state,
            effects,
        })
    }

    pub fn evaluate_helper(
        &self,
        state: &KernelState,
        helper_name: &str,
        args: &BTreeMap<FieldId, KernelValue>,
    ) -> Result<KernelValue, TransitionRefusal> {
        let helper_transition = helper_transition_id();
        let helper = self
            .schema
            .helpers
            .iter()
            .chain(self.schema.derived.iter())
            .find(|candidate| candidate.name == helper_name)
            .ok_or_else(|| TransitionRefusal::EvaluationError {
                machine: self.schema.machine.clone(),
                transition: helper_transition.clone(),
                reason: format!("unknown helper `{helper_name}`"),
            })?;

        let mut bindings = BTreeMap::new();
        for param in &helper.params {
            let Some(value) = args.get(&param.name) else {
                return Err(TransitionRefusal::EvaluationError {
                    machine: self.schema.machine.clone(),
                    transition: helper_transition.clone(),
                    reason: format!("missing helper arg `{}`", param.name),
                });
            };
            if !value_matches_type(value, &param.ty) {
                return Err(TransitionRefusal::EvaluationError {
                    machine: self.schema.machine.clone(),
                    transition: helper_transition.clone(),
                    reason: format!("helper arg `{}` does not match declared type", param.name),
                });
            }
            bindings.insert(param.name.as_str().to_owned(), value.clone());
        }

        self.eval_helper(state, &bindings, helper, &helper_transition)
    }

    fn render_effect(
        &self,
        state: &KernelState,
        bindings: &BTreeMap<String, KernelValue>,
        effect: &EffectEmit,
        transition_name: &TransitionId,
    ) -> Result<KernelEffect, TransitionRefusal> {
        let mut fields = BTreeMap::new();
        for (name, expr) in &effect.fields {
            fields.insert(
                name.clone(),
                self.eval_expr(state, bindings, expr, transition_name)?,
            );
        }
        Ok(KernelEffect {
            variant: effect.variant.clone(),
            fields,
        })
    }

    fn apply_update(
        &self,
        state: &mut KernelState,
        bindings: &BTreeMap<String, KernelValue>,
        update: &Update,
        transition_name: &TransitionId,
    ) -> Result<(), TransitionRefusal> {
        match update {
            Update::Assign { field, expr } => {
                let value = self.eval_expr(state, bindings, expr, transition_name)?;
                state.fields.insert(field.clone(), value);
            }
            Update::Increment { field, amount } => {
                let current = state
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or(KernelValue::U64(0));
                let value = current
                    .as_u64()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                state.fields.insert(
                    field.clone(),
                    KernelValue::U64(value.saturating_add(*amount)),
                );
            }
            Update::Decrement { field, amount } => {
                let current = state
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or(KernelValue::U64(0));
                let value = current
                    .as_u64()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let next = value.checked_sub(*amount).ok_or_else(|| {
                    self.eval_error(transition_name, format!("underflow decrementing `{field}`"))
                })?;
                state.fields.insert(field.clone(), KernelValue::U64(next));
            }
            Update::MapInsert { field, key, value } => {
                let key = self.eval_expr(state, bindings, key, transition_name)?;
                let value = self.eval_expr(state, bindings, value, transition_name)?;
                let mut map = state
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or(KernelValue::Map(BTreeMap::new()))
                    .into_map()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                map.insert(key, value);
                state.fields.insert(field.clone(), KernelValue::Map(map));
            }
            Update::MapRemove { field, key } => {
                let key = self.eval_expr(state, bindings, key, transition_name)?;
                let mut map = state
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or(KernelValue::Map(BTreeMap::new()))
                    .into_map()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                map.remove(&key);
                state.fields.insert(field.clone(), KernelValue::Map(map));
            }
            Update::MapIncrement { field, key, amount } => {
                let key = self.eval_expr(state, bindings, key, transition_name)?;
                let mut map = state
                    .fields
                    .get(field)
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
                state.fields.insert(field.clone(), KernelValue::Map(map));
            }
            Update::MapDecrement { field, key, amount } => {
                let key = self.eval_expr(state, bindings, key, transition_name)?;
                let mut map = state
                    .fields
                    .get(field)
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
                state.fields.insert(field.clone(), KernelValue::Map(map));
            }
            Update::SetInsert { field, value } => {
                let value = self.eval_expr(state, bindings, value, transition_name)?;
                let mut set = state
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or(KernelValue::Set(BTreeSet::new()))
                    .into_set()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                set.insert(value);
                state.fields.insert(field.clone(), KernelValue::Set(set));
            }
            Update::SetRemove { field, value } => {
                let value = self.eval_expr(state, bindings, value, transition_name)?;
                let mut set = state
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or(KernelValue::Set(BTreeSet::new()))
                    .into_set()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                set.remove(&value);
                state.fields.insert(field.clone(), KernelValue::Set(set));
            }
            Update::SeqAppend { field, value } => {
                let value = self.eval_expr(state, bindings, value, transition_name)?;
                let mut seq = state
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or(KernelValue::Seq(Vec::new()))
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                seq.push(value);
                state.fields.insert(field.clone(), KernelValue::Seq(seq));
            }
            Update::SeqPrepend { field, values } => {
                let values = self.eval_expr(state, bindings, values, transition_name)?;
                let mut prefix = values
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let mut seq = state
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or(KernelValue::Seq(Vec::new()))
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                prefix.append(&mut seq);
                state.fields.insert(field.clone(), KernelValue::Seq(prefix));
            }
            Update::SeqPopFront { field } => {
                let mut seq = state
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or(KernelValue::Seq(Vec::new()))
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                if !seq.is_empty() {
                    seq.remove(0);
                }
                state.fields.insert(field.clone(), KernelValue::Seq(seq));
            }
            Update::SeqRemoveValue { field, value } => {
                let value = self.eval_expr(state, bindings, value, transition_name)?;
                let mut seq = state
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or(KernelValue::Seq(Vec::new()))
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                if let Some(index) = seq.iter().position(|item| item == &value) {
                    seq.remove(index);
                }
                state.fields.insert(field.clone(), KernelValue::Seq(seq));
            }
            Update::SeqRemoveAll { field, values } => {
                let values = self.eval_expr(state, bindings, values, transition_name)?;
                let values = values
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                let mut seq = state
                    .fields
                    .get(field)
                    .cloned()
                    .unwrap_or(KernelValue::Seq(Vec::new()))
                    .into_seq()
                    .map_err(|reason| self.eval_error(transition_name, reason))?;
                for value in values {
                    if let Some(index) = seq.iter().position(|item| item == &value) {
                        seq.remove(index);
                    }
                }
                state.fields.insert(field.clone(), KernelValue::Seq(seq));
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
        transition_name: &TransitionId,
    ) -> Result<KernelValue, TransitionRefusal> {
        match expr {
            Expr::Bool(value) => Ok(KernelValue::Bool(*value)),
            Expr::U64(value) => Ok(KernelValue::U64(*value)),
            Expr::String(value) => Ok(KernelValue::String(value.clone())),
            Expr::NamedVariant { enum_name, variant } => Ok(KernelValue::NamedVariant {
                enum_name: enum_name.clone(),
                variant: variant.clone(),
            }),
            Expr::EmptySet => Ok(KernelValue::Set(BTreeSet::new())),
            Expr::EmptyMap => Ok(KernelValue::Map(BTreeMap::new())),
            Expr::SeqLiteral(items) => Ok(KernelValue::Seq(
                items
                    .iter()
                    .map(|item| self.eval_expr(state, bindings, item, transition_name))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            Expr::CurrentPhase => Ok(KernelValue::String(state.phase.as_str().to_owned())),
            Expr::Phase(phase) => Ok(KernelValue::String(phase.as_str().to_owned())),
            Expr::Field(field) => state.fields.get(field).cloned().ok_or_else(|| {
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
                    nested_bindings.insert(
                        param.name.as_str().to_owned(),
                        self.eval_expr(state, bindings, arg, transition_name)?,
                    );
                }
                self.eval_helper(state, &nested_bindings, helper, transition_name)
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
        transition_name: &TransitionId,
    ) -> Result<KernelValue, TransitionRefusal> {
        self.eval_expr(state, bindings, &helper.body, transition_name)
    }

    fn compare_values(
        &self,
        state: &KernelState,
        bindings: &BTreeMap<String, KernelValue>,
        left: &Expr,
        right: &Expr,
        transition_name: &TransitionId,
        predicate: impl FnOnce(&KernelValue, &KernelValue) -> bool,
    ) -> Result<KernelValue, TransitionRefusal> {
        let left = self.eval_expr(state, bindings, left, transition_name)?;
        let right = self.eval_expr(state, bindings, right, transition_name)?;
        Ok(KernelValue::Bool(predicate(&left, &right)))
    }

    fn eval_error(
        &self,
        transition_name: &TransitionId,
        reason: impl Into<String>,
    ) -> TransitionRefusal {
        TransitionRefusal::EvaluationError {
            machine: self.schema.machine.clone(),
            transition: transition_name.clone(),
            reason: reason.into(),
        }
    }
}

/// Synthetic transition id used in error contexts that are not raised from a
/// concrete transition (schema-init field evaluation, helper evaluation).
///
/// `<init>` and `<helper>` are not valid identity slugs, so they are threaded
/// through via a dedicated sentinel built at call sites that need one. The
/// slug `__init_helper__` is valid under [`PhaseId`]-style rules (underscores
/// and alphanumerics only) and reserved for this purpose.
fn helper_transition_id() -> TransitionId {
    #[allow(clippy::expect_used)]
    {
        TransitionId::parse("__init_helper__").expect("reserved helper sentinel slug")
    }
}

fn trigger_match_matches(on: &TriggerMatch, trigger: &RouteVariantId) -> bool {
    match (on, trigger) {
        (TriggerMatch::Input { variant, .. }, RouteVariantId::Input(other)) => variant == other,
        (TriggerMatch::Signal { variant, .. }, RouteVariantId::Signal(other)) => variant == other,
        _ => false,
    }
}

#[allow(dead_code)]
pub(crate) fn initial_state_from_schema(schema: MachineSchema) -> Result<RawState, RawRefusal> {
    GeneratedMachineKernel::new(schema).initial_state()
}

#[allow(dead_code)]
pub(crate) fn transition_from_schema(
    schema: MachineSchema,
    state: &RawState,
    input: &RawInput,
) -> Result<RawOutcome, RawRefusal> {
    GeneratedMachineKernel::new(schema).transition(state, input)
}

#[allow(dead_code)]
pub(crate) fn transition_signal_from_schema(
    schema: MachineSchema,
    state: &RawState,
    signal: &RawSignal,
) -> Result<RawOutcome, RawRefusal> {
    GeneratedMachineKernel::new(schema).transition_signal(state, signal)
}

#[allow(dead_code)]
pub(crate) fn evaluate_helper_from_schema(
    schema: MachineSchema,
    state: &RawState,
    helper_name: &str,
    args: &std::collections::BTreeMap<FieldId, RawValue>,
) -> Result<RawValue, RawRefusal> {
    GeneratedMachineKernel::new(schema).evaluate_helper(state, helper_name, args)
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
            Self::String(value) => Ok(value.as_str()),
            other => Err(format!("expected string, found {other:?}")),
        }
    }

    pub fn as_named_variant(&self, expected_enum: &str) -> Result<&str, String> {
        match self {
            Self::NamedVariant { enum_name, variant } if enum_name.as_str() == expected_enum => {
                Ok(variant.as_str())
            }
            Self::NamedVariant { enum_name, .. } => Err(format!(
                "expected named variant of enum `{expected_enum}`, found enum `{enum_name}`"
            )),
            other => Err(format!("expected named variant, found {other:?}")),
        }
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
        TypeRef::Named(name) if named_type_is_u64(name.as_str()) => KernelValue::U64(0),
        TypeRef::Named(_) => KernelValue::String(String::new()),
        TypeRef::Enum(name) =>
        {
            #[allow(clippy::expect_used)]
            KernelValue::NamedVariant {
                enum_name: name.clone(),
                variant: EnumVariantId::parse("_Unset").expect("reserved default enum variant"),
            }
        }
        TypeRef::Option(_) => KernelValue::None,
        TypeRef::Set(_) => KernelValue::Set(BTreeSet::new()),
        TypeRef::Seq(_) => KernelValue::Seq(Vec::new()),
        TypeRef::Map(_, _) => KernelValue::Map(BTreeMap::new()),
    }
}

fn value_matches_type(value: &KernelValue, ty: &TypeRef) -> bool {
    match (value, ty) {
        (KernelValue::Bool(_), TypeRef::Bool) => true,
        (KernelValue::U64(_), TypeRef::U32 | TypeRef::U64) => true,
        (KernelValue::String(_), TypeRef::String) => true,
        (KernelValue::U64(_), TypeRef::Named(name)) if named_type_is_u64(name.as_str()) => true,
        (KernelValue::String(_), TypeRef::Named(name)) if !named_type_is_u64(name.as_str()) => true,
        (KernelValue::NamedVariant { enum_name, .. }, TypeRef::Enum(name)) if enum_name == name => {
            true
        }
        (KernelValue::None, TypeRef::Option(_)) => true,
        (inner, TypeRef::Option(inner_ty)) => {
            value_matches_type(inner, inner_ty) || option_map_matches_inner(inner, inner_ty)
        }
        (KernelValue::Set(values), TypeRef::Set(inner_ty)) => values
            .iter()
            .all(|value| value_matches_type(value, inner_ty)),
        (KernelValue::Seq(values), TypeRef::Seq(inner_ty)) => values
            .iter()
            .all(|value| value_matches_type(value, inner_ty)),
        (KernelValue::Map(values), TypeRef::Map(key_ty, value_ty)) => {
            values.iter().all(|(key, value)| {
                value_matches_type(key, key_ty) && value_matches_type(value, value_ty)
            })
        }
        _ => false,
    }
}

fn named_type_is_u64(name: &str) -> bool {
    matches!(
        name,
        "BoundarySequence" | "TurnNumber" | "FenceToken" | "Generation"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use meerkat_machine_schema::canonical_machine_schemas;
    use meerkat_machine_schema::catalog::dsl::{
        dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
    };
    use meerkat_machine_schema::identity::{
        EffectVariantId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, PhaseId,
        SignalVariantId, TransitionId,
    };

    use super::{
        GeneratedMachineKernel, KernelInput, KernelSignal, KernelValue, TransitionRefusal,
    };

    fn input_id(slug: &str) -> InputVariantId {
        #[allow(clippy::expect_used)]
        InputVariantId::parse(slug).expect("valid input variant slug")
    }

    fn signal_id(slug: &str) -> SignalVariantId {
        #[allow(clippy::expect_used)]
        SignalVariantId::parse(slug).expect("valid signal variant slug")
    }

    fn field_id(slug: &str) -> FieldId {
        #[allow(clippy::expect_used)]
        FieldId::parse(slug).expect("valid field slug")
    }

    fn phase_id(slug: &str) -> PhaseId {
        #[allow(clippy::expect_used)]
        PhaseId::parse(slug).expect("valid phase slug")
    }

    fn effect_id(slug: &str) -> EffectVariantId {
        #[allow(clippy::expect_used)]
        EffectVariantId::parse(slug).expect("valid effect slug")
    }

    fn transition_id(slug: &str) -> TransitionId {
        #[allow(clippy::expect_used)]
        TransitionId::parse(slug).expect("valid transition slug")
    }

    fn enum_type_id(slug: &str) -> EnumTypeId {
        #[allow(clippy::expect_used)]
        EnumTypeId::parse(slug).expect("valid enum type slug")
    }

    fn enum_variant_id(slug: &str) -> EnumVariantId {
        #[allow(clippy::expect_used)]
        EnumVariantId::parse(slug).expect("valid enum variant slug")
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
                    variant: input_id("DoesNotExist"),
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
                    variant: signal_id("Initialize"),
                    fields: BTreeMap::new(),
                },
            )
            .expect("initialize");
        let registered = kernel
            .transition(
                &initialized.next_state,
                &KernelInput {
                    variant: input_id("RegisterSession"),
                    fields: BTreeMap::from([(
                        field_id("session_id"),
                        KernelValue::String("session-1".into()),
                    )]),
                },
            )
            .expect("register session");
        let refusal = kernel
            .transition(
                &registered.next_state,
                &KernelInput {
                    variant: input_id("PrepareBindings"),
                    fields: BTreeMap::from([
                        (field_id("agent_runtime_id"), KernelValue::U64(7)),
                        (field_id("fence_token"), KernelValue::U64(3)),
                        (field_id("generation"), KernelValue::U64(1)),
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
        assert_eq!(state.phase, phase_id("Running"));
        let running = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: input_id("Spawn"),
                    fields: BTreeMap::from([
                        (
                            field_id("agent_identity"),
                            KernelValue::String("agent.worker".into()),
                        ),
                        (
                            field_id("agent_runtime_id"),
                            KernelValue::String("runtime.worker.1".into()),
                        ),
                        (field_id("fence_token"), KernelValue::U64(41)),
                        (field_id("generation"), KernelValue::U64(2)),
                        (field_id("external_addressable"), KernelValue::Bool(false)),
                        // W3-H-1: new realtime-binding fields. `replacing`
                        // is None (no prior binding) so the Fresh branch
                        // fires.
                        (
                            field_id("bridge_session_id"),
                            KernelValue::String("bridge.worker.1".into()),
                        ),
                        (field_id("replacing"), KernelValue::None),
                    ]),
                },
            )
            .expect("spawn member");
        assert_eq!(running.transition, transition_id("SpawnRunningFresh"));
        assert_eq!(running.next_state.phase, phase_id("Running"));
        assert!(
            running
                .effects
                .iter()
                .any(|effect| effect.variant == effect_id("RequestRuntimeBinding"))
        );
        let submitted = kernel
            .transition(
                &running.next_state,
                &KernelInput {
                    variant: input_id("SubmitWork"),
                    fields: BTreeMap::from([
                        (
                            field_id("agent_runtime_id"),
                            KernelValue::String("runtime.worker.1".into()),
                        ),
                        (field_id("fence_token"), KernelValue::U64(41)),
                        (field_id("work_id"), KernelValue::String("work-1".into())),
                        (
                            field_id("origin"),
                            KernelValue::NamedVariant {
                                enum_name: enum_type_id("WorkOrigin"),
                                variant: enum_variant_id("Internal"),
                            },
                        ),
                    ]),
                },
            )
            .expect("submit work");
        assert_eq!(
            submitted.transition,
            transition_id("SubmitWorkRunningInternal")
        );
        // SubmitMemberWork effect was removed (unimplemented route to MeerkatMachine).
        assert!(
            !submitted
                .effects
                .iter()
                .any(|effect| effect.variant == effect_id("SubmitMemberWork"))
        );
        assert_eq!(
            submitted
                .next_state
                .fields
                .get(&field_id("active_run_count")),
            Some(&KernelValue::U64(0))
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
}
