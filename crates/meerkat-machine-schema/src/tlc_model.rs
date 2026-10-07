//! Inert finite model fixtures. They do not change generated runtime semantics.
//! Values are validated against the same named structural bindings as the owner.

use crate::identity::{FieldId, NamedTypeId};
use crate::{NamedTypeBinding, RustTypeAtom, TypePathEnumPayloadAtom, TypePathStructFieldAtom};
use std::collections::BTreeMap;

// TLC IntValue uses signed 32-bit integers; u64::MAX is the existing named abstraction.
const TLC_LITERAL_MAX: u64 = 2_147_483_647;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlcValue {
    Bool(bool),
    U64(u64),
    String(String),
    None,
    Some(Box<Self>),
    Record(BTreeMap<FieldId, Self>),
    Set(Vec<Self>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineTlcProfile {
    /// Complete values for standalone named CONSTANT domains. Nested record
    /// values remain explicit; this is not a recursive override mechanism.
    pub named_values: BTreeMap<NamedTypeId, Vec<TlcValue>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineTlcStateLimits {
    pub step_limit: u32,
    pub seq_limit: u32,
    pub set_limit: u32,
    pub map_limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MachineTlcModel {
    pub ci: MachineTlcProfile,
    pub deep: MachineTlcProfile,
    pub ci_limits: MachineTlcStateLimits,
    pub require_ci_transition_coverage: bool,
}

impl MachineTlcModel {
    pub(crate) fn validate(&self, bindings: &[NamedTypeBinding]) -> Result<(), String> {
        if self.ci_limits.step_limit == 0 {
            return Err("CI step limit must be positive".into());
        }
        for limit in [
            self.ci_limits.step_limit,
            self.ci_limits.seq_limit,
            self.ci_limits.set_limit,
            self.ci_limits.map_limit,
        ] {
            if u64::from(limit) > TLC_LITERAL_MAX {
                return Err("TLC state limit exceeds supported signed 32-bit literal range".into());
            }
        }
        for profile in [&self.ci, &self.deep] {
            for (name, values) in &profile.named_values {
                if values.is_empty() || has_duplicates(values) {
                    return Err(format!(
                        "model domain {name} is empty or contains duplicates"
                    ));
                }
                for value in values {
                    validate_named(value, name, bindings, 0)?;
                }
            }
        }
        Ok(())
    }
}

fn has_duplicates(values: &[TlcValue]) -> bool {
    values
        .iter()
        .enumerate()
        .any(|(index, value)| values[..index].contains(value))
}

fn validate_named(
    value: &TlcValue,
    name: &NamedTypeId,
    bindings: &[NamedTypeBinding],
    depth: usize,
) -> Result<(), String> {
    if depth > 32 {
        return Err("model value exceeds structural nesting limit".into());
    }
    if matches!(value, TlcValue::U64(number) if *number > TLC_LITERAL_MAX && *number != u64::MAX) {
        return Err("model integer exceeds supported TLC literal range".into());
    }
    let binding = bindings
        .iter()
        .find(|binding| binding.name == *name)
        .ok_or_else(|| format!("model domain refers to unknown named type {name}"))?;
    let valid = match (&binding.rust, value) {
        (RustTypeAtom::U64, TlcValue::U64(_)) | (RustTypeAtom::Bool, TlcValue::Bool(_)) => true,
        (RustTypeAtom::U32, TlcValue::U64(v)) => u32::try_from(*v).is_ok(),
        (RustTypeAtom::U16, TlcValue::U64(v)) => u16::try_from(*v).is_ok(),
        (RustTypeAtom::U8, TlcValue::U64(v)) => u8::try_from(*v).is_ok(),
        (RustTypeAtom::String | RustTypeAtom::TypePath(_), TlcValue::String(_)) => true,
        (RustTypeAtom::StringEnum { variants }, TlcValue::String(v)) => variants.iter().any(|item| item.as_str() == v),
        (RustTypeAtom::TypePathFieldPresenceSet { fields, .. }, TlcValue::Set(values)) => {
            !has_duplicates(values) && values.iter().all(|v| matches!(v, TlcValue::String(s) if fields.iter().any(|field| field.as_str() == s)))
        }
        (RustTypeAtom::TypePathStruct { fields, .. }, TlcValue::Record(values)) => {
            if fields.len() != values.len() { return Err(format!("model record {name} has missing or unknown fields")); }
            for field in fields {
                let v = values.get(&field.name).ok_or_else(|| format!("model record {name} is missing {}", field.name))?;
                match (&field.atom, v) {
                    (TypePathStructFieldAtom::String, TlcValue::String(_)) | (TypePathStructFieldAtom::OptionalNamed(_), TlcValue::None) => {},
                    (TypePathStructFieldAtom::Named(inner), v) => validate_named(v, inner, bindings, depth + 1)?,
                    (TypePathStructFieldAtom::OptionalNamed(inner), TlcValue::Some(v)) => validate_named(v, inner, bindings, depth + 1)?,
                    _ => return Err(format!("model record {name}.{} has incorrect value shape", field.name)),
                }
            }
            true
        }
        (RustTypeAtom::TypePathEnum { unit_variants, structural_variants, .. }, v) => {
            if structural_variants.is_empty() {
                matches!(v, TlcValue::String(s) if unit_variants.iter().any(|item| item.as_str() == s))
            } else if let TlcValue::Record(values) = v {
                let tag = values.iter().find(|(field, _)| field.as_str() == "tag").map(|(_, value)| value);
                let Some(TlcValue::String(tag)) = tag else { return Err(format!("model enum {name} requires a string tag")); };
                if unit_variants.iter().any(|item| item.as_str() == tag) { values.len() == 1 }
                else if let Some(variant) = structural_variants.iter().find(|v| v.variant.as_str() == tag) {
                    if values.len() != variant.fields.len() + 1 { return Err(format!("model enum {name} has missing or unknown fields")); }
                    for field in &variant.fields {
                        let v = values.get(&field.name).ok_or_else(|| format!("model enum {name} missing {}", field.name))?;
                        match (&field.atom, v) {
                            (TypePathEnumPayloadAtom::String, TlcValue::String(_)) | (TypePathEnumPayloadAtom::OptionalString, TlcValue::None) => {},
                            (TypePathEnumPayloadAtom::OptionalString, TlcValue::Some(v)) if matches!(v.as_ref(), TlcValue::String(_)) => {},
                            (TypePathEnumPayloadAtom::Named(inner), v) => validate_named(v, inner, bindings, depth + 1)?,
                            (TypePathEnumPayloadAtom::NamedSet(inner), TlcValue::Set(values)) if !has_duplicates(values) => {
                                for v in values { validate_named(v, inner, bindings, depth + 1)?; }
                            }
                            (TypePathEnumPayloadAtom::StringSet, TlcValue::Set(values)) if !has_duplicates(values) && values.iter().all(|v| matches!(v, TlcValue::String(_))) => {},
                            _ => return Err(format!("model enum {name}.{} has incorrect value shape", field.name)),
                        }
                    }
                    true
                } else { false }
            } else { false }
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(format!("model value does not match named type {name}"))
    }
}
