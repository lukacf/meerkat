use std::collections::{BTreeMap, BTreeSet};

use meerkat_machine_schema::{
    EffectEmit, Expr, HelperSchema, MachineSchema, Quantifier, TransitionSchema, TypeRef, Update,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum KernelValue {
    Bool(bool),
    U64(u64),
    String(String),
    NamedVariant { enum_name: String, variant: String },
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
        enum_name: String,
        variant: String,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelState {
    pub phase: String,
    pub fields: BTreeMap<String, KernelValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelInput {
    pub variant: String,
    pub fields: BTreeMap<String, KernelValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelEffect {
    pub variant: String,
    pub fields: BTreeMap<String, KernelValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionOutcome {
    pub transition: String,
    pub next_state: KernelState,
    pub effects: Vec<KernelEffect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TransitionRefusal {
    #[error("unknown input variant `{variant}` for machine `{machine}`")]
    UnknownInputVariant { machine: String, variant: String },
    #[error("invalid input payload for machine `{machine}` variant `{variant}`: {reason}")]
    InvalidInputPayload {
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

impl GeneratedMachineKernel {
    #[must_use]
    pub fn new(schema: MachineSchema) -> Self {
        Self { schema }
    }

    #[must_use]
    pub fn schema(&self) -> &MachineSchema {
        &self.schema
    }

    pub fn initial_state(&self) -> Result<KernelState, TransitionRefusal> {
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
            let value = self.eval_expr(&state, &BTreeMap::new(), &init.expr, "<init>")?;
            state.fields.insert(init.field.clone(), value);
        }

        Ok(state)
    }

    pub fn transition(
        &self,
        state: &KernelState,
        input: &KernelInput,
    ) -> Result<TransitionOutcome, TransitionRefusal> {
        let input_variant = self
            .schema
            .inputs
            .variant_named(&input.variant)
            .map_err(|_| TransitionRefusal::UnknownInputVariant {
                machine: self.schema.machine.clone(),
                variant: input.variant.clone(),
            })?;

        for field in &input_variant.fields {
            let Some(value) = input.fields.get(&field.name) else {
                return Err(TransitionRefusal::InvalidInputPayload {
                    machine: self.schema.machine.clone(),
                    variant: input.variant.clone(),
                    reason: format!("missing field `{}`", field.name),
                });
            };
            if !value_matches_type(value, &field.ty) {
                return Err(TransitionRefusal::InvalidInputPayload {
                    machine: self.schema.machine.clone(),
                    variant: input.variant.clone(),
                    reason: format!("field `{}` does not match declared type", field.name),
                });
            }
        }

        let mut matches = Vec::new();
        for transition in &self.schema.transitions {
            if !transition.from.iter().any(|phase| phase == &state.phase) {
                continue;
            }
            if transition.on.variant != input.variant {
                continue;
            }

            let mut bindings = BTreeMap::new();
            let mut malformed = false;
            for binding in &transition.on.bindings {
                let Some(value) = input.fields.get(binding) else {
                    malformed = true;
                    break;
                };
                bindings.insert(binding.clone(), value.clone());
            }
            if malformed {
                return Err(TransitionRefusal::InvalidInputPayload {
                    machine: self.schema.machine.clone(),
                    variant: input.variant.clone(),
                    reason: "transition binding missing from payload".into(),
                });
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
                variant: input.variant.clone(),
            }),
            1 => {
                let Some((transition, bindings)) = matches.pop() else {
                    return Err(TransitionRefusal::NoMatchingTransition {
                        machine: self.schema.machine.clone(),
                        phase: state.phase.clone(),
                        variant: input.variant.clone(),
                    });
                };
                self.apply_transition(state, transition, &bindings)
            }
            _ => Err(TransitionRefusal::AmbiguousTransition {
                machine: self.schema.machine.clone(),
                phase: state.phase.clone(),
                variant: input.variant.clone(),
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
        args: &BTreeMap<String, KernelValue>,
    ) -> Result<KernelValue, TransitionRefusal> {
        let helper = self
            .schema
            .helpers
            .iter()
            .chain(self.schema.derived.iter())
            .find(|candidate| candidate.name == helper_name)
            .ok_or_else(|| TransitionRefusal::EvaluationError {
                machine: self.schema.machine.clone(),
                transition: "<helper>".to_string(),
                reason: format!("unknown helper `{helper_name}`"),
            })?;

        let mut bindings = BTreeMap::new();
        for param in &helper.params {
            let Some(value) = args.get(&param.name) else {
                return Err(TransitionRefusal::EvaluationError {
                    machine: self.schema.machine.clone(),
                    transition: "<helper>".to_string(),
                    reason: format!("missing helper arg `{}`", param.name),
                });
            };
            if !value_matches_type(value, &param.ty) {
                return Err(TransitionRefusal::EvaluationError {
                    machine: self.schema.machine.clone(),
                    transition: "<helper>".to_string(),
                    reason: format!("helper arg `{}` does not match declared type", param.name),
                });
            }
            bindings.insert(param.name.clone(), value.clone());
        }

        self.eval_helper(state, &bindings, helper, "<helper>")
    }

    fn render_effect(
        &self,
        state: &KernelState,
        bindings: &BTreeMap<String, KernelValue>,
        effect: &EffectEmit,
        transition_name: &str,
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
        transition_name: &str,
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
        transition_name: &str,
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
            Expr::CurrentPhase => Ok(KernelValue::String(state.phase.clone())),
            Expr::Phase(phase) => Ok(KernelValue::String(phase.clone())),
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
            Expr::Some(inner) => self.eval_expr(state, bindings, inner, transition_name),
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
                        param.name.clone(),
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
            Self::String(value) => Ok(value.as_str()),
            other => Err(format!("expected string, found {other:?}")),
        }
    }

    pub fn as_named_variant(&self, expected_enum: &str) -> Result<&str, String> {
        match self {
            Self::NamedVariant { enum_name, variant } if enum_name == expected_enum => {
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
        TypeRef::Named(name) if named_type_is_u64(name) => KernelValue::U64(0),
        TypeRef::Named(_) => KernelValue::String(String::new()),
        TypeRef::Enum(name) => KernelValue::NamedVariant {
            enum_name: name.clone(),
            variant: String::new(),
        },
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
        (KernelValue::U64(_), TypeRef::Named(name)) if named_type_is_u64(name) => true,
        (KernelValue::String(_), TypeRef::Named(name)) if !named_type_is_u64(name) => true,
        (KernelValue::NamedVariant { enum_name, .. }, TypeRef::Enum(name)) if enum_name == name => {
            true
        }
        (KernelValue::None, TypeRef::Option(_)) => true,
        (inner, TypeRef::Option(inner_ty)) => value_matches_type(inner, inner_ty),
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
    matches!(name, "BoundarySequence" | "TurnNumber")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use meerkat_machine_schema::{
        canonical_machine_schemas, input_lifecycle_machine, runtime_control_machine,
    };

    use super::{
        GeneratedMachineKernel, KernelInput, KernelValue, TransitionOutcome, TransitionRefusal,
    };

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
    fn input_lifecycle_queue_accepted_transition_executes() {
        let kernel = GeneratedMachineKernel::new(input_lifecycle_machine());
        let state = kernel.initial_state().expect("initial state");
        let outcome = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "QueueAccepted".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("queue accepted transition");
        assert_eq!(outcome.transition, "QueueAccepted");
        assert_eq!(outcome.next_state.phase, "Queued");
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn runtime_control_rejects_unknown_input_variant() {
        let kernel = GeneratedMachineKernel::new(runtime_control_machine());
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
    fn input_payload_types_are_checked() {
        let kernel = GeneratedMachineKernel::new(runtime_control_machine());
        let state = kernel.initial_state().expect("initial state");
        let refusal = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "SubmitWork".into(),
                    fields: BTreeMap::from([
                        ("work_id".into(), KernelValue::String("work-1".into())),
                        ("content_shape".into(), KernelValue::U64(99)),
                        ("handling_mode".into(), KernelValue::String("Queue".into())),
                        ("request_id".into(), KernelValue::None),
                        ("reservation_key".into(), KernelValue::None),
                    ]),
                },
            )
            .expect_err("typed payload mismatch should fail");
        assert!(matches!(
            refusal,
            TransitionRefusal::InvalidInputPayload { .. }
        ));
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_run_rejects_string_payloads_for_enum_fields() {
        use meerkat_machine_schema::flow_run_machine;

        let kernel = GeneratedMachineKernel::new(flow_run_machine());
        let state = kernel.initial_state().expect("initial state");
        let refusal = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "CreateRun".into(),
                    fields: BTreeMap::from([
                        (
                            "step_ids".into(),
                            KernelValue::Seq(vec![KernelValue::String("step-a".into())]),
                        ),
                        (
                            "ordered_steps".into(),
                            KernelValue::Seq(vec![KernelValue::String("step-a".into())]),
                        ),
                        (
                            "step_has_conditions".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::Bool(false),
                            )])),
                        ),
                        (
                            "step_dependencies".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::Seq(vec![]),
                            )])),
                        ),
                        (
                            "step_dependency_modes".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::NamedVariant {
                                    enum_name: "DependencyMode".into(),
                                    variant: "All".into(),
                                },
                            )])),
                        ),
                        (
                            "step_branches".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::None,
                            )])),
                        ),
                        (
                            "step_collection_policies".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::String("All".into()),
                            )])),
                        ),
                        (
                            "step_quorum_thresholds".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::U64(0),
                            )])),
                        ),
                        ("escalation_threshold".into(), KernelValue::U64(0)),
                        ("max_step_retries".into(), KernelValue::U64(0)),
                    ]),
                },
            )
            .expect_err("string enum payload should fail");
        assert!(matches!(
            refusal,
            TransitionRefusal::InvalidInputPayload { .. }
        ));
    }

    fn flow_named_variant(enum_name: &str, variant: &str) -> KernelValue {
        KernelValue::NamedVariant {
            enum_name: enum_name.into(),
            variant: variant.into(),
        }
    }

    fn flow_create_run_input(
        step_ids: &[&str],
        step_has_conditions: &[(&str, bool)],
        step_dependencies: &[(&str, &[&str])],
        step_dependency_modes: &[(&str, &str)],
        step_branches: &[(&str, Option<&str>)],
    ) -> KernelInput {
        let ordered_steps = step_ids
            .iter()
            .map(|step_id| KernelValue::String((*step_id).into()))
            .collect::<Vec<_>>();
        let step_ids = ordered_steps.clone();
        let step_has_conditions = step_has_conditions
            .iter()
            .map(|(step_id, has_conditions)| {
                (
                    KernelValue::String((*step_id).into()),
                    KernelValue::Bool(*has_conditions),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let step_dependencies = step_dependencies
            .iter()
            .map(|(step_id, dependencies)| {
                (
                    KernelValue::String((*step_id).into()),
                    KernelValue::Seq(
                        dependencies
                            .iter()
                            .map(|dependency| KernelValue::String((*dependency).into()))
                            .collect::<Vec<_>>(),
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let step_dependency_modes = step_dependency_modes
            .iter()
            .map(|(step_id, mode)| {
                (
                    KernelValue::String((*step_id).into()),
                    flow_named_variant("DependencyMode", mode),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let step_branches = step_branches
            .iter()
            .map(|(step_id, branch)| {
                (
                    KernelValue::String((*step_id).into()),
                    match branch {
                        Some(branch_id) => KernelValue::String((*branch_id).into()),
                        None => KernelValue::None,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let step_collection_policies = step_dependency_modes
            .keys()
            .map(|step_id| {
                (
                    step_id.clone(),
                    flow_named_variant("CollectionPolicyKind", "All"),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let step_quorum_thresholds = step_dependency_modes
            .keys()
            .map(|step_id| (step_id.clone(), KernelValue::U64(0)))
            .collect::<BTreeMap<_, _>>();

        KernelInput {
            variant: "CreateRun".into(),
            fields: BTreeMap::from([
                ("step_ids".into(), KernelValue::Seq(step_ids)),
                ("ordered_steps".into(), KernelValue::Seq(ordered_steps)),
                (
                    "step_has_conditions".into(),
                    KernelValue::Map(step_has_conditions),
                ),
                (
                    "step_dependencies".into(),
                    KernelValue::Map(step_dependencies),
                ),
                (
                    "step_dependency_modes".into(),
                    KernelValue::Map(step_dependency_modes),
                ),
                ("step_branches".into(), KernelValue::Map(step_branches)),
                (
                    "step_collection_policies".into(),
                    KernelValue::Map(step_collection_policies),
                ),
                (
                    "step_quorum_thresholds".into(),
                    KernelValue::Map(step_quorum_thresholds),
                ),
                ("escalation_threshold".into(), KernelValue::U64(0)),
                ("max_step_retries".into(), KernelValue::U64(0)),
            ]),
        }
    }

    fn flow_step_input(variant: &str, step_id: &str) -> KernelInput {
        KernelInput {
            variant: variant.into(),
            fields: BTreeMap::from([("step_id".into(), KernelValue::String(step_id.into()))]),
        }
    }

    fn flow_has_effect(outcome: &TransitionOutcome, variant: &str) -> bool {
        outcome
            .effects
            .iter()
            .any(|effect| effect.variant == variant)
    }

    #[allow(clippy::expect_used, clippy::panic)]
    fn flow_helper_bool(
        kernel: &GeneratedMachineKernel,
        state: &super::KernelState,
        helper_name: &str,
        step_id: &str,
    ) -> bool {
        match kernel
            .evaluate_helper(
                state,
                helper_name,
                &BTreeMap::from([("step_id".into(), KernelValue::String(step_id.into()))]),
            )
            .expect("helper evaluation should succeed")
        {
            KernelValue::Bool(value) => value,
            other => panic!("expected bool helper result, got {other:?}"),
        }
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_run_dispatch_requires_recorded_true_condition() {
        use meerkat_machine_schema::flow_run_machine;

        let kernel = GeneratedMachineKernel::new(flow_run_machine());
        let state = kernel.initial_state().expect("initial state");
        let created = kernel
            .transition(
                &state,
                &flow_create_run_input(
                    &["conditional"],
                    &[("conditional", true)],
                    &[("conditional", &[])],
                    &[("conditional", "All")],
                    &[("conditional", None)],
                ),
            )
            .expect("create run");
        let running = kernel
            .transition(
                &created.next_state,
                &KernelInput {
                    variant: "StartRun".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("start run");

        let refusal = kernel
            .transition(
                &running.next_state,
                &flow_step_input("DispatchStep", "conditional"),
            )
            .expect_err("dispatch should be refused before condition result");
        assert!(matches!(
            refusal,
            TransitionRefusal::NoMatchingTransition { .. }
        ));

        let condition_passed = kernel
            .transition(
                &running.next_state,
                &flow_step_input("ConditionPassed", "conditional"),
            )
            .expect("record condition");
        let dispatched = kernel
            .transition(
                &condition_passed.next_state,
                &flow_step_input("DispatchStep", "conditional"),
            )
            .expect("dispatch after condition passed");
        assert_eq!(dispatched.transition, "DispatchStep");
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_run_dispatch_requires_dependencies_to_be_ready() {
        use meerkat_machine_schema::flow_run_machine;

        let kernel = GeneratedMachineKernel::new(flow_run_machine());
        let state = kernel.initial_state().expect("initial state");
        let created = kernel
            .transition(
                &state,
                &flow_create_run_input(
                    &["dep", "gated"],
                    &[("dep", false), ("gated", false)],
                    &[("dep", &[]), ("gated", &["dep"])],
                    &[("dep", "All"), ("gated", "All")],
                    &[("dep", None), ("gated", None)],
                ),
            )
            .expect("create run");
        let running = kernel
            .transition(
                &created.next_state,
                &KernelInput {
                    variant: "StartRun".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("start run");

        let refusal = kernel
            .transition(
                &running.next_state,
                &flow_step_input("DispatchStep", "gated"),
            )
            .expect_err("dispatch should be refused before dependency completes");
        assert!(matches!(
            refusal,
            TransitionRefusal::NoMatchingTransition { .. }
        ));

        let dep_dispatched = kernel
            .transition(&running.next_state, &flow_step_input("DispatchStep", "dep"))
            .expect("dispatch dependency");
        let dep_completed = kernel
            .transition(
                &dep_dispatched.next_state,
                &flow_step_input("CompleteStep", "dep"),
            )
            .expect("complete dependency");
        let gated_dispatched = kernel
            .transition(
                &dep_completed.next_state,
                &flow_step_input("DispatchStep", "gated"),
            )
            .expect("dispatch gated step after dependency");
        assert_eq!(gated_dispatched.transition, "DispatchStep");
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_run_dispatch_blocks_losing_branch_after_winner_completes() {
        use meerkat_machine_schema::flow_run_machine;

        let kernel = GeneratedMachineKernel::new(flow_run_machine());
        let state = kernel.initial_state().expect("initial state");
        let created = kernel
            .transition(
                &state,
                &flow_create_run_input(
                    &["first", "second"],
                    &[("first", false), ("second", false)],
                    &[("first", &[]), ("second", &[])],
                    &[("first", "All"), ("second", "All")],
                    &[("first", Some("winner")), ("second", Some("winner"))],
                ),
            )
            .expect("create run");
        let running = kernel
            .transition(
                &created.next_state,
                &KernelInput {
                    variant: "StartRun".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("start run");
        let first_dispatched = kernel
            .transition(
                &running.next_state,
                &flow_step_input("DispatchStep", "first"),
            )
            .expect("dispatch first");
        let first_completed = kernel
            .transition(
                &first_dispatched.next_state,
                &flow_step_input("CompleteStep", "first"),
            )
            .expect("complete first");
        let refusal = kernel
            .transition(
                &first_completed.next_state,
                &flow_step_input("DispatchStep", "second"),
            )
            .expect_err("dispatch should be refused once branch winner completed");
        assert!(matches!(
            refusal,
            TransitionRefusal::NoMatchingTransition { .. }
        ));
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_run_condition_rejection_skips_step_and_blocks_later_dispatch() {
        use meerkat_machine_schema::flow_run_machine;

        let kernel = GeneratedMachineKernel::new(flow_run_machine());
        let state = kernel.initial_state().expect("initial state");
        let created = kernel
            .transition(
                &state,
                &flow_create_run_input(
                    &["conditional"],
                    &[("conditional", true)],
                    &[("conditional", &[])],
                    &[("conditional", "All")],
                    &[("conditional", None)],
                ),
            )
            .expect("create run");
        let running = kernel
            .transition(
                &created.next_state,
                &KernelInput {
                    variant: "StartRun".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("start run");
        let rejected = kernel
            .transition(
                &running.next_state,
                &flow_step_input("ConditionRejected", "conditional"),
            )
            .expect("reject condition");
        assert!(flow_has_effect(&rejected, "EmitStepNotice"));

        let refusal = kernel
            .transition(
                &rejected.next_state,
                &flow_step_input("DispatchStep", "conditional"),
            )
            .expect_err("skipped step should no longer dispatch");
        assert!(matches!(
            refusal,
            TransitionRefusal::NoMatchingTransition { .. }
        ));
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_run_fail_step_emits_supervisor_escalation_at_threshold() {
        use meerkat_machine_schema::flow_run_machine;

        let kernel = GeneratedMachineKernel::new(flow_run_machine());
        let state = kernel.initial_state().expect("initial state");
        let created = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "CreateRun".into(),
                    fields: BTreeMap::from([
                        (
                            "step_ids".into(),
                            KernelValue::Seq(vec![KernelValue::String("step-a".into())]),
                        ),
                        (
                            "ordered_steps".into(),
                            KernelValue::Seq(vec![KernelValue::String("step-a".into())]),
                        ),
                        (
                            "step_has_conditions".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::Bool(false),
                            )])),
                        ),
                        (
                            "step_dependencies".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::Seq(vec![]),
                            )])),
                        ),
                        (
                            "step_dependency_modes".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                flow_named_variant("DependencyMode", "All"),
                            )])),
                        ),
                        (
                            "step_branches".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::None,
                            )])),
                        ),
                        (
                            "step_collection_policies".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                flow_named_variant("CollectionPolicyKind", "All"),
                            )])),
                        ),
                        (
                            "step_quorum_thresholds".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::U64(0),
                            )])),
                        ),
                        ("escalation_threshold".into(), KernelValue::U64(1)),
                        ("max_step_retries".into(), KernelValue::U64(0)),
                    ]),
                },
            )
            .expect("create run");
        let running = kernel
            .transition(
                &created.next_state,
                &KernelInput {
                    variant: "StartRun".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("start run");
        let dispatched = kernel
            .transition(
                &running.next_state,
                &flow_step_input("DispatchStep", "step-a"),
            )
            .expect("dispatch step");
        let failed = kernel
            .transition(
                &dispatched.next_state,
                &flow_step_input("FailStep", "step-a"),
            )
            .expect("fail step");
        assert!(flow_has_effect(&failed, "AppendFailureLedger"));
        assert!(flow_has_effect(&failed, "EscalateSupervisor"));
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_run_any_dependency_all_skipped_sets_skip_truth() {
        use meerkat_machine_schema::flow_run_machine;

        let kernel = GeneratedMachineKernel::new(flow_run_machine());
        let state = kernel.initial_state().expect("initial state");
        let created = kernel
            .transition(
                &state,
                &flow_create_run_input(
                    &["dep_a", "dep_b", "gated"],
                    &[("dep_a", false), ("dep_b", false), ("gated", false)],
                    &[
                        ("dep_a", &[]),
                        ("dep_b", &[]),
                        ("gated", &["dep_a", "dep_b"]),
                    ],
                    &[("dep_a", "All"), ("dep_b", "All"), ("gated", "Any")],
                    &[("dep_a", None), ("dep_b", None), ("gated", None)],
                ),
            )
            .expect("create run");
        let running = kernel
            .transition(
                &created.next_state,
                &KernelInput {
                    variant: "StartRun".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("start run");
        let dep_a_skipped = kernel
            .transition(&running.next_state, &flow_step_input("SkipStep", "dep_a"))
            .expect("skip dep_a");
        let dep_b_skipped = kernel
            .transition(
                &dep_a_skipped.next_state,
                &flow_step_input("SkipStep", "dep_b"),
            )
            .expect("skip dep_b");

        assert!(flow_helper_bool(
            &kernel,
            &dep_b_skipped.next_state,
            "StepDependencyShouldSkip",
            "gated",
        ));
        assert!(!flow_helper_bool(
            &kernel,
            &dep_b_skipped.next_state,
            "StepDependencyReady",
            "gated",
        ));
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn flow_run_quorum_collection_truth_is_machine_derived() {
        use meerkat_machine_schema::flow_run_machine;

        let kernel = GeneratedMachineKernel::new(flow_run_machine());
        let state = kernel.initial_state().expect("initial state");
        let created = kernel
            .transition(
                &state,
                &KernelInput {
                    variant: "CreateRun".into(),
                    fields: BTreeMap::from([
                        (
                            "step_ids".into(),
                            KernelValue::Seq(vec![KernelValue::String("step-a".into())]),
                        ),
                        (
                            "ordered_steps".into(),
                            KernelValue::Seq(vec![KernelValue::String("step-a".into())]),
                        ),
                        (
                            "step_has_conditions".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::Bool(false),
                            )])),
                        ),
                        (
                            "step_dependencies".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::Seq(vec![]),
                            )])),
                        ),
                        (
                            "step_dependency_modes".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                flow_named_variant("DependencyMode", "All"),
                            )])),
                        ),
                        (
                            "step_branches".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::None,
                            )])),
                        ),
                        (
                            "step_collection_policies".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                flow_named_variant("CollectionPolicyKind", "Quorum"),
                            )])),
                        ),
                        (
                            "step_quorum_thresholds".into(),
                            KernelValue::Map(BTreeMap::from([(
                                KernelValue::String("step-a".into()),
                                KernelValue::U64(2),
                            )])),
                        ),
                        ("escalation_threshold".into(), KernelValue::U64(0)),
                        ("max_step_retries".into(), KernelValue::U64(0)),
                    ]),
                },
            )
            .expect("create run");
        let running = kernel
            .transition(
                &created.next_state,
                &KernelInput {
                    variant: "StartRun".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("start run");
        let registered = kernel
            .transition(
                &running.next_state,
                &KernelInput {
                    variant: "RegisterTargets".into(),
                    fields: BTreeMap::from([
                        ("step_id".into(), KernelValue::String("step-a".into())),
                        ("target_count".into(), KernelValue::U64(3)),
                    ]),
                },
            )
            .expect("register targets");
        let dispatched = kernel
            .transition(
                &registered.next_state,
                &flow_step_input("DispatchStep", "step-a"),
            )
            .expect("dispatch step");
        let first_success = kernel
            .transition(
                &dispatched.next_state,
                &KernelInput {
                    variant: "RecordTargetSuccess".into(),
                    fields: BTreeMap::from([
                        ("step_id".into(), KernelValue::String("step-a".into())),
                        ("target_id".into(), KernelValue::String("worker-1".into())),
                    ]),
                },
            )
            .expect("record first success");
        assert!(flow_helper_bool(
            &kernel,
            &first_success.next_state,
            "CollectionFeasible",
            "step-a",
        ));
        assert!(!flow_helper_bool(
            &kernel,
            &first_success.next_state,
            "CollectionSatisfied",
            "step-a",
        ));
        let second_success = kernel
            .transition(
                &first_success.next_state,
                &KernelInput {
                    variant: "RecordTargetSuccess".into(),
                    fields: BTreeMap::from([
                        ("step_id".into(), KernelValue::String("step-a".into())),
                        ("target_id".into(), KernelValue::String("worker-2".into())),
                    ]),
                },
            )
            .expect("record second success");
        assert!(flow_helper_bool(
            &kernel,
            &second_success.next_state,
            "CollectionSatisfied",
            "step-a",
        ));
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn named_numeric_aliases_accept_u64_payloads() {
        use meerkat_machine_schema::{external_tool_surface_machine, input_lifecycle_machine};

        let external_tool_kernel = GeneratedMachineKernel::new(external_tool_surface_machine());
        let external_tool_state = external_tool_kernel.initial_state().expect("initial state");
        let staged_add = external_tool_kernel
            .transition(
                &external_tool_state,
                &KernelInput {
                    variant: "StageAdd".into(),
                    fields: BTreeMap::from([(
                        "surface_id".into(),
                        KernelValue::String("alpha".into()),
                    )]),
                },
            )
            .expect("stage add");
        let applied = external_tool_kernel
            .transition(
                &staged_add.next_state,
                &KernelInput {
                    variant: "ApplyBoundary".into(),
                    fields: BTreeMap::from([
                        ("surface_id".into(), KernelValue::String("alpha".into())),
                        ("applied_at_turn".into(), KernelValue::U64(7)),
                    ]),
                },
            )
            .expect("turn-number payload should be accepted");
        assert_eq!(applied.transition, "ApplyBoundaryAdd");

        let input_lifecycle_kernel = GeneratedMachineKernel::new(input_lifecycle_machine());
        let input_lifecycle_state = input_lifecycle_kernel
            .initial_state()
            .expect("initial state");
        let queued = input_lifecycle_kernel
            .transition(
                &input_lifecycle_state,
                &KernelInput {
                    variant: "QueueAccepted".into(),
                    fields: BTreeMap::new(),
                },
            )
            .expect("queue accepted");
        let staged = input_lifecycle_kernel
            .transition(
                &queued.next_state,
                &KernelInput {
                    variant: "StageForRun".into(),
                    fields: BTreeMap::from([(
                        "run_id".into(),
                        KernelValue::String("run-1".into()),
                    )]),
                },
            )
            .expect("stage for run");
        let applied = input_lifecycle_kernel
            .transition(
                &staged.next_state,
                &KernelInput {
                    variant: "MarkApplied".into(),
                    fields: BTreeMap::from([(
                        "run_id".into(),
                        KernelValue::String("run-1".into()),
                    )]),
                },
            )
            .expect("mark applied");
        let boundary_marked = input_lifecycle_kernel
            .transition(
                &applied.next_state,
                &KernelInput {
                    variant: "MarkAppliedPendingConsumption".into(),
                    fields: BTreeMap::from([("boundary_sequence".into(), KernelValue::U64(1))]),
                },
            )
            .expect("boundary-sequence payload should be accepted");
        assert_eq!(boundary_marked.transition, "MarkAppliedPendingConsumption");
    }

    #[allow(clippy::expect_used)]
    #[test]
    fn kernel_value_map_roundtrips_through_json() {
        let value = KernelValue::Map(BTreeMap::from([
            (
                KernelValue::String("step-a".into()),
                KernelValue::NamedVariant {
                    enum_name: "StepRunStatus".into(),
                    variant: "Completed".into(),
                },
            ),
            (
                KernelValue::String("step-b".into()),
                KernelValue::Seq(vec![
                    KernelValue::String("dep-1".into()),
                    KernelValue::String("dep-2".into()),
                ]),
            ),
        ]));

        let encoded = serde_json::to_string(&value).expect("serialize kernel value");
        let decoded: KernelValue =
            serde_json::from_str(&encoded).expect("deserialize kernel value");

        assert_eq!(decoded, value);
    }
}
