use std::collections::{BTreeMap, BTreeSet};

use meerkat_machine_schema::{
    EffectEmit, Expr, HelperSchema, MachineSchema, Quantifier, TransitionSchema, TypeRef, Update,
};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum KernelValue {
    Bool(bool),
    U64(u64),
    String(String),
    Seq(Vec<KernelValue>),
    Set(BTreeSet<KernelValue>),
    Map(BTreeMap<KernelValue, KernelValue>),
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
                let (transition, bindings) = matches.pop().expect("single match");
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
                    KernelValue::Bool(_) | KernelValue::U64(_) | KernelValue::None => false,
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
            Expr::Len(inner) => {
                let inner = self.eval_expr(state, bindings, inner, transition_name)?;
                let len = match inner {
                    KernelValue::Seq(items) => items.len(),
                    KernelValue::Set(items) => items.len(),
                    KernelValue::Map(items) => items.len(),
                    KernelValue::String(items) => items.chars().count(),
                    KernelValue::Bool(_) | KernelValue::U64(_) | KernelValue::None => {
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

    use super::{GeneratedMachineKernel, KernelInput, KernelValue, TransitionRefusal};

    #[test]
    fn every_catalog_machine_builds_an_initial_state() {
        for schema in canonical_machine_schemas() {
            let kernel = GeneratedMachineKernel::new(schema.clone());
            let state = kernel.initial_state().expect("initial state");
            assert_eq!(state.phase, schema.state.init.phase);
        }
    }

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
}
