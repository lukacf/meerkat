//! The test oracle's native helpers against every canonical machine schema.
//!
//! A native helper is a helper a schema calls without declaring it: its
//! semantics live in Rust next to the DSL, and rustc holds every DSL call
//! site to that Rust signature. The generated kernel is the one consumer
//! nothing held to those call sites, so a DSL change could move a native's
//! arguments while the kernel kept the old ones, and every row that reached
//! the native went blind.
//!
//! This walks every native helper call in every canonical machine schema:
//! transition guards, updates and effects, helper and derived bodies,
//! invariants and state initializers. A call must name a helper the kernel
//! implements, with the argument count `KERNEL_NATIVE_HELPERS` lists, or a
//! helper `KERNEL_UNIMPLEMENTED_NATIVE_HELPERS` lists with the reason it is
//! still missing. A gap entry that the kernel implements or no schema calls
//! fails, and so does a kernel arm no schema calls: both lists track the
//! schemas exactly.

#![cfg(feature = "test-oracle")]

use std::collections::{BTreeMap, BTreeSet};

use meerkat_machine_kernels::test_oracle::{
    KERNEL_NATIVE_HELPERS, KERNEL_UNIMPLEMENTED_NATIVE_HELPERS,
};
use meerkat_machine_schema::{Expr, MachineSchema, Update, canonical_machine_schemas};

/// One native helper call: where it is and how many arguments it passes.
struct NativeCall {
    site: String,
    arity: usize,
}

/// Collects the native helper calls of one schema into a shared map keyed by
/// helper name.
struct NativeCallWalker<'schema, 'calls> {
    declared: BTreeSet<&'schema str>,
    site: String,
    calls: &'calls mut BTreeMap<String, Vec<NativeCall>>,
}

impl NativeCallWalker<'_, '_> {
    // Exhaustive on purpose: a new expression form must say whether it can
    // hold a helper call.
    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Bool(_)
            | Expr::U64(_)
            | Expr::U64Max
            | Expr::String(_)
            | Expr::NamedVariant { .. }
            | Expr::EmptySet
            | Expr::EmptyMap
            | Expr::CurrentPhase
            | Expr::Phase(_)
            | Expr::Field(_)
            | Expr::Binding(_)
            | Expr::Variant(_)
            | Expr::None => {}
            Expr::FieldAccess { base: inner, .. }
            | Expr::EnumVariantIs { value: inner, .. }
            | Expr::EnumStringSetPayload { value: inner, .. }
            | Expr::Not(inner)
            | Expr::SeqElements(inner)
            | Expr::Len(inner)
            | Expr::Head(inner)
            | Expr::MapKeys(inner)
            | Expr::Some(inner) => self.expr(inner),
            Expr::SeqLiteral(items) | Expr::And(items) | Expr::Or(items) => {
                for item in items {
                    self.expr(item);
                }
            }
            Expr::IfElse {
                condition,
                then_expr,
                else_expr,
            } => {
                self.expr(condition);
                self.expr(then_expr);
                self.expr(else_expr);
            }
            Expr::Eq(left, right)
            | Expr::Neq(left, right)
            | Expr::Add(left, right)
            | Expr::Sub(left, right)
            | Expr::Gt(left, right)
            | Expr::Gte(left, right)
            | Expr::Lt(left, right)
            | Expr::Lte(left, right)
            | Expr::Contains {
                collection: left,
                value: right,
            }
            | Expr::Count {
                collection: left,
                value: right,
            }
            | Expr::MapContainsKey {
                map: left,
                key: right,
            }
            | Expr::MapGet {
                map: left,
                key: right,
            }
            | Expr::MapValue {
                map: left,
                key: right,
            }
            | Expr::SeqStartsWith {
                seq: left,
                prefix: right,
            }
            | Expr::Quantified {
                over: left,
                body: right,
                ..
            } => {
                self.expr(left);
                self.expr(right);
            }
            Expr::MapLiteral(entries) => {
                for (key, value) in entries {
                    self.expr(key);
                    self.expr(value);
                }
            }
            Expr::Call { helper, args } => {
                if !self.declared.contains(helper.as_str()) {
                    self.calls
                        .entry(helper.clone())
                        .or_default()
                        .push(NativeCall {
                            site: self.site.clone(),
                            arity: args.len(),
                        });
                }
                for arg in args {
                    self.expr(arg);
                }
            }
        }
    }

    // Exhaustive for the same reason as `expr`.
    fn update(&mut self, update: &Update) {
        match update {
            Update::Increment { .. } | Update::Decrement { .. } | Update::SeqPopFront { .. } => {}
            Update::Assign { expr, .. }
            | Update::MapIncrement { key: expr, .. }
            | Update::MapDecrement { key: expr, .. }
            | Update::MapRemove { key: expr, .. }
            | Update::SetInsert { value: expr, .. }
            | Update::SetRemove { value: expr, .. }
            | Update::SeqAppend { value: expr, .. }
            | Update::SeqPrepend { values: expr, .. }
            | Update::SeqRemoveValue { value: expr, .. }
            | Update::SeqRemoveAll { values: expr, .. } => self.expr(expr),
            Update::MapInsert { key, value, .. } => {
                self.expr(key);
                self.expr(value);
            }
            Update::Conditional {
                condition,
                then_updates,
                else_updates,
            } => {
                self.expr(condition);
                for nested in then_updates.iter().chain(else_updates) {
                    self.update(nested);
                }
            }
            Update::ForEach { over, updates, .. } => {
                self.expr(over);
                for nested in updates {
                    self.update(nested);
                }
            }
        }
    }
}

/// Adds every native helper call of `schema` to `calls`.
fn collect_native_calls(schema: &MachineSchema, calls: &mut BTreeMap<String, Vec<NativeCall>>) {
    let machine = schema.machine.as_str();
    let mut walker = NativeCallWalker {
        declared: schema
            .helpers
            .iter()
            .chain(&schema.derived)
            .map(|helper| helper.name.as_str())
            .collect(),
        site: String::new(),
        calls,
    };
    for init in &schema.state.init.fields {
        walker.site = format!("{machine} initializer of `{}`", init.field);
        walker.expr(&init.expr);
    }
    for invariant in &schema.invariants {
        walker.site = format!("{machine} invariant `{}`", invariant.name);
        walker.expr(&invariant.expr);
    }
    for helper in schema.helpers.iter().chain(&schema.derived) {
        walker.site = format!("{machine} helper `{}`", helper.name);
        walker.expr(&helper.body);
    }
    for transition in &schema.transitions {
        let name = &transition.name;
        for guard in &transition.guards {
            walker.site = format!("{machine} transition `{name}` guard `{}`", guard.name);
            walker.expr(&guard.expr);
        }
        walker.site = format!("{machine} transition `{name}` update");
        for update in &transition.updates {
            walker.update(update);
        }
        for emit in &transition.emit {
            walker.site = format!("{machine} transition `{name}` effect `{}`", emit.variant);
            for expr in emit.fields.values() {
                walker.expr(expr);
            }
        }
    }
}

#[test]
fn every_native_helper_call_matches_the_kernel_table_or_a_reasoned_gap() {
    let mut calls = BTreeMap::new();
    for schema in canonical_machine_schemas() {
        collect_native_calls(&schema, &mut calls);
    }
    let implemented: BTreeMap<&str, usize> = KERNEL_NATIVE_HELPERS.iter().copied().collect();
    let gaps: BTreeMap<&str, &str> = KERNEL_UNIMPLEMENTED_NATIVE_HELPERS
        .iter()
        .copied()
        .collect();
    let mut failures = Vec::new();

    if implemented.len() != KERNEL_NATIVE_HELPERS.len() {
        failures.push("KERNEL_NATIVE_HELPERS lists a helper twice".to_owned());
    }
    if gaps.len() != KERNEL_UNIMPLEMENTED_NATIVE_HELPERS.len() {
        failures.push("KERNEL_UNIMPLEMENTED_NATIVE_HELPERS lists a helper twice".to_owned());
    }

    for (helper, sites) in &calls {
        match implemented.get(&helper.as_str()) {
            Some(&arity) => {
                for call in sites.iter().filter(|call| call.arity != arity) {
                    failures.push(format!(
                        "{}: `{helper}` is called with {} args, the kernel implements it with \
                         {arity} (KERNEL_NATIVE_HELPERS)",
                        call.site, call.arity
                    ));
                }
            }
            None if gaps.contains_key(&helper.as_str()) => {}
            None => failures.push(format!(
                "`{helper}` is called {} times, first by {}, and has neither a kernel arm in \
                 KERNEL_NATIVE_HELPERS nor an entry with a reason in \
                 KERNEL_UNIMPLEMENTED_NATIVE_HELPERS",
                sites.len(),
                sites.first().map_or("", |call| call.site.as_str())
            )),
        }
    }

    for &(helper, _) in KERNEL_NATIVE_HELPERS {
        if !calls.contains_key(helper) {
            failures.push(format!(
                "kernel arm `{helper}` is called by no canonical machine schema: remove it from \
                 KERNEL_NATIVE_HELPERS and the dispatch, or restore its call"
            ));
        }
    }

    for &(helper, reason) in KERNEL_UNIMPLEMENTED_NATIVE_HELPERS {
        if reason.trim().is_empty() {
            failures.push(format!("gap `{helper}` has no written reason"));
        }
        if implemented.contains_key(&helper) {
            failures.push(format!(
                "stale gap `{helper}`: the kernel implements it, remove it from \
                 KERNEL_UNIMPLEMENTED_NATIVE_HELPERS"
            ));
        }
        if !calls.contains_key(helper) {
            failures.push(format!(
                "stale gap `{helper}`: no canonical machine schema calls it, remove it from \
                 KERNEL_UNIMPLEMENTED_NATIVE_HELPERS"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "native helper calls and the test oracle disagree:\n{}",
        failures.join("\n")
    );
}
