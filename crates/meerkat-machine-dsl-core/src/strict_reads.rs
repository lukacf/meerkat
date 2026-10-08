//! Strict map reads in guards (#1811).
//!
//! A guard's value-projected map read, `self.m.get_cloned(k).get("value")`
//! (and the `get` / `get_copied` forms), used to lower to a Rust
//! `unwrap_or_default()` while TLA+ projected the absent case to the string
//! `"none"`. The two disagreed whenever the compared value could equal the
//! type's default: Rust took an arm that TLC treated as disabled. This pass
//! rewrites every such read in a transition guard to [`ExprDef::MapValue`],
//! which every executor treats as a strict read: TLA+ applies the function
//! (an absent key is a TLC error on any reachable evaluation), the generated
//! mutator refuses the input with `AbsentMapKey`, and the protocol
//! authorities use their fail-closed `*_value()` accessors.
//!
//! Reads inside a quantifier body are left as they are: their key usually
//! names the quantifier binding, and a refusal there would not mean "this
//! input read an absent key".

use crate::ast::{ExprDef, MachineDef};

/// Rewrite every value-projected state-map read in every transition guard.
pub(crate) fn normalize_guard_value_reads(def: &mut MachineDef) {
    for transition in &mut def.transitions {
        for guard in &mut transition.guards {
            rewrite(&mut guard.expr);
        }
    }
}

/// Whether `expr` contains a strict read (outside quantifier bodies, which
/// the normalization never rewrites).
pub(crate) fn contains_map_value(expr: &ExprDef) -> bool {
    if matches!(expr, ExprDef::MapValue { .. }) {
        return true;
    }
    children(expr).into_iter().any(contains_map_value)
}

/// The state-map read under a `.get("value")` projection, if `expr` is one.
fn value_projection(expr: &ExprDef) -> Option<(&ExprDef, &ExprDef)> {
    let ExprDef::MapGet { map, key } = expr else {
        return None;
    };
    if !matches!(key.as_ref(), ExprDef::StringLit(projected) if projected == "value") {
        return None;
    }
    let (inner_map, inner_key) = match map.as_ref() {
        ExprDef::MapGet { map, key }
        | ExprDef::MapGetCopied { map, key }
        | ExprDef::MapGetCloned { map, key } => (map.as_ref(), key.as_ref()),
        _ => return None,
    };
    matches!(inner_map, ExprDef::Field(_)).then_some((inner_map, inner_key))
}

fn rewrite(expr: &mut ExprDef) {
    if let Some((map, key)) = value_projection(expr) {
        let mut key = key.clone();
        rewrite(&mut key);
        *expr = ExprDef::MapValue {
            map: Box::new(map.clone()),
            key: Box::new(key),
        };
        return;
    }
    match expr {
        // Quantifier bodies are deliberately left lenient (module docs).
        ExprDef::ForAll { over, .. } | ExprDef::Exists { over, .. } => rewrite(over),
        _ => {
            for child in children_mut(expr) {
                rewrite(child);
            }
        }
    }
}

/// Direct sub-expressions, written exhaustively so a new variant must be
/// classified here.
fn children(expr: &ExprDef) -> Vec<&ExprDef> {
    match expr {
        ExprDef::Bool(_)
        | ExprDef::U64(_)
        | ExprDef::U64Max
        | ExprDef::StringLit(_)
        | ExprDef::None
        | ExprDef::EmptySeq
        | ExprDef::EmptySet
        | ExprDef::EmptyMap
        | ExprDef::Field(_)
        | ExprDef::Binding(_)
        | ExprDef::CurrentPhase
        | ExprDef::Phase(_)
        | ExprDef::NamedVariant { .. } => Vec::new(),
        ExprDef::Some(inner)
        | ExprDef::Not(inner)
        | ExprDef::Len(inner)
        | ExprDef::MapKeys(inner)
        | ExprDef::IsSome(inner)
        | ExprDef::IsNone(inner)
        | ExprDef::FieldAccess { base: inner, .. }
        | ExprDef::EnumVariantIs { value: inner, .. }
        | ExprDef::EnumStringSetPayload { value: inner, .. } => vec![inner.as_ref()],
        ExprDef::And(items) | ExprDef::Or(items) | ExprDef::Call { args: items, .. } => {
            items.iter().collect()
        }
        ExprDef::Eq(l, r)
        | ExprDef::Neq(l, r)
        | ExprDef::Gt(l, r)
        | ExprDef::Gte(l, r)
        | ExprDef::Lt(l, r)
        | ExprDef::Lte(l, r)
        | ExprDef::Add(l, r)
        | ExprDef::Sub(l, r) => vec![l.as_ref(), r.as_ref()],
        ExprDef::Contains {
            collection: l,
            value: r,
        }
        | ExprDef::Count {
            collection: l,
            value: r,
        }
        | ExprDef::MapContainsKey { map: l, key: r }
        | ExprDef::MapGet { map: l, key: r }
        | ExprDef::MapGetCopied { map: l, key: r }
        | ExprDef::MapGetCloned { map: l, key: r }
        | ExprDef::MapValue { map: l, key: r } => vec![l.as_ref(), r.as_ref()],
        // A quantifier body is never rewritten, so only its domain counts.
        ExprDef::ForAll { over, .. } | ExprDef::Exists { over, .. } => vec![over.as_ref()],
        ExprDef::IfElse {
            condition,
            then_expr,
            else_expr,
        } => vec![condition.as_ref(), then_expr.as_ref(), else_expr.as_ref()],
    }
}

/// Mutable direct sub-expressions (quantifier bodies excluded), exhaustive
/// like [`children`].
fn children_mut(expr: &mut ExprDef) -> Vec<&mut ExprDef> {
    match expr {
        ExprDef::Bool(_)
        | ExprDef::U64(_)
        | ExprDef::U64Max
        | ExprDef::StringLit(_)
        | ExprDef::None
        | ExprDef::EmptySeq
        | ExprDef::EmptySet
        | ExprDef::EmptyMap
        | ExprDef::Field(_)
        | ExprDef::Binding(_)
        | ExprDef::CurrentPhase
        | ExprDef::Phase(_)
        | ExprDef::NamedVariant { .. } => Vec::new(),
        ExprDef::Some(inner)
        | ExprDef::Not(inner)
        | ExprDef::Len(inner)
        | ExprDef::MapKeys(inner)
        | ExprDef::IsSome(inner)
        | ExprDef::IsNone(inner)
        | ExprDef::FieldAccess { base: inner, .. }
        | ExprDef::EnumVariantIs { value: inner, .. }
        | ExprDef::EnumStringSetPayload { value: inner, .. } => vec![inner.as_mut()],
        ExprDef::And(items) | ExprDef::Or(items) | ExprDef::Call { args: items, .. } => {
            items.iter_mut().collect()
        }
        ExprDef::Eq(l, r)
        | ExprDef::Neq(l, r)
        | ExprDef::Gt(l, r)
        | ExprDef::Gte(l, r)
        | ExprDef::Lt(l, r)
        | ExprDef::Lte(l, r)
        | ExprDef::Add(l, r)
        | ExprDef::Sub(l, r) => vec![l.as_mut(), r.as_mut()],
        ExprDef::Contains {
            collection: l,
            value: r,
        }
        | ExprDef::Count {
            collection: l,
            value: r,
        }
        | ExprDef::MapContainsKey { map: l, key: r }
        | ExprDef::MapGet { map: l, key: r }
        | ExprDef::MapGetCopied { map: l, key: r }
        | ExprDef::MapGetCloned { map: l, key: r }
        | ExprDef::MapValue { map: l, key: r } => vec![l.as_mut(), r.as_mut()],
        ExprDef::ForAll { over, .. } | ExprDef::Exists { over, .. } => vec![over.as_mut()],
        ExprDef::IfElse {
            condition,
            then_expr,
            else_expr,
        } => vec![condition.as_mut(), then_expr.as_mut(), else_expr.as_mut()],
    }
}

/// The lazy definedness predicate of `expr`: true exactly when evaluating
/// `expr` left to right, with the short-circuiting of `&&`, `||` and
/// if/else, never reads a strict map key that is absent. `None` means the
/// expression reads no strict key (trivially defined).
///
/// Every executor conjoins this before the guard (`Defined(G) && G`), so an
/// absent key refuses the input instead of erroring or defaulting: TLA+
/// disables the action, the generated mutator misses the arm, and the
/// protocol authorities' fail-closed accessors are never reached with an
/// absent key (#1811).
pub(crate) fn definedness(expr: &ExprDef) -> Option<ExprDef> {
    match expr {
        ExprDef::MapValue { map, key } => Some(and(
            definedness(key),
            ExprDef::MapContainsKey {
                map: map.clone(),
                key: key.clone(),
            },
        )),
        ExprDef::And(items) => lazy_and(items),
        ExprDef::Or(items) => lazy_or(items),
        ExprDef::Not(inner) => definedness(inner),
        ExprDef::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            let branches = match (definedness(then_expr), definedness(else_expr)) {
                (None, None) => None,
                (then_d, else_d) => Some(ExprDef::IfElse {
                    condition: condition.clone(),
                    then_expr: Box::new(then_d.unwrap_or(ExprDef::Bool(true))),
                    else_expr: Box::new(else_d.unwrap_or(ExprDef::Bool(true))),
                }),
            };
            conj(definedness(condition), branches)
        }
        // Quantifier bodies are never rewritten; only the domain counts.
        ExprDef::ForAll { over, .. } | ExprDef::Exists { over, .. } => definedness(over),
        // Every other form evaluates all of its operands.
        _ => children(expr)
            .into_iter()
            .fold(None, |acc, child| conj(acc, definedness(child))),
    }
}

/// `Defined(a1 && a2 && ...)` = `Defined(a1) && (!a1 || Defined(a2 && ...))`.
fn lazy_and(items: &[ExprDef]) -> Option<ExprDef> {
    let (first, rest) = items.split_first()?;
    let rest_d = lazy_and(rest);
    let tail =
        rest_d.map(|rest_d| ExprDef::Or(vec![ExprDef::Not(Box::new(first.clone())), rest_d]));
    conj(definedness(first), tail)
}

/// `Defined(a1 || a2 || ...)` = `Defined(a1) && (a1 || Defined(a2 || ...))`.
fn lazy_or(items: &[ExprDef]) -> Option<ExprDef> {
    let (first, rest) = items.split_first()?;
    let rest_d = lazy_or(rest);
    let tail = rest_d.map(|rest_d| ExprDef::Or(vec![first.clone(), rest_d]));
    conj(definedness(first), tail)
}

fn conj(left: Option<ExprDef>, right: Option<ExprDef>) -> Option<ExprDef> {
    match (left, right) {
        (None, None) => None,
        (Some(only), None) | (None, Some(only)) => Some(only),
        (Some(left), Some(right)) => Some(ExprDef::And(vec![left, right])),
    }
}

fn and(left: Option<ExprDef>, right: ExprDef) -> ExprDef {
    match left {
        None => right,
        Some(left) => ExprDef::And(vec![left, right]),
    }
}

/// The state-map field name of the first strict read in `expr`, for the
/// `AbsentMapKey` diagnostic.
pub(crate) fn first_map_value_field(expr: &ExprDef) -> Option<String> {
    if let ExprDef::MapValue { map, .. } = expr
        && let ExprDef::Field(name) = map.as_ref()
    {
        return Some(name.to_string());
    }
    children(expr).into_iter().find_map(first_map_value_field)
}
