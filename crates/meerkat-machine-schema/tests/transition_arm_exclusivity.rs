#![allow(clippy::expect_used, clippy::panic, clippy::print_stdout)]

//! Same-trigger arms must be mutually exclusive (#1811, part b).
//!
//! For one (phase, trigger) the generated mutator dispatches its arms as an
//! `if / else if` chain in declaration order. TLA+'s `Next` is a disjunction,
//! and the interpreter kernel refuses two matching arms as
//! `AmbiguousTransition`. When two guards can hold together, declaration
//! order silently decides what production does, while the model treats the
//! input as nondeterministic.
//!
//! This contract groups every canonical machine's arms by source phase and
//! trigger. Each pair in a group must be refuted structurally: one conjunct
//! of each arm, and the two cannot both hold. The rules are sound and
//! deliberately conservative:
//! - distinct constants of the same kind for the same subject;
//! - `x` against `!x`, after `x == true` / `x == false` normalization;
//! - `==` against `!=`, and order complements (`a < b` against `b <= a`);
//! - case splits through `||`;
//! - helper calls inlined with capture-avoiding substitution;
//! - the lowered `m.get_cloned(k)` option view, rewritten to membership plus
//!   the stored value;
//! - definedness of strict map reads (`MapValue`, #1811 part a), which every
//!   executor refuses for an absent key, so an arm that reads `m[k]` strictly
//!   only fires with `k` present.
//!
//! Lenient reads (an `Option` field's `.get("value")`, reads inside quantifier
//! bodies) imply nothing, because the generated mutator still defaults them.
//!
//! A pair these rules cannot refute must be listed in [`BASELINE`] with its
//! reason:
//! - `invariant:<name>`: disjoint by an invariant the machine declares, which
//!   TLC checks;
//! - `unproven-1811`: not yet classified.
//!
//! The contract fails on a new unrefuted pair, on a stale entry (now
//! refutable, or its arms are gone), and on an invariant reason the machine
//! does not declare. The baseline therefore only shrinks.
//!
//! To update it after a reviewed machine change, run `cargo test --test
//! transition_arm_exclusivity -- --ignored print_exclusivity_baseline
//! --nocapture`. It prints the current unrefuted pairs on stdout (the census
//! summary goes to stderr), keeping each listed reason and printing a new
//! pair as `unproven-1811`. Replace [`BASELINE`] with those lines. The review
//! list is then the `git diff` of [`BASELINE`], and every `+` line needs a
//! decision: a guard that makes the pair refutable, an `invariant:<name>` the
//! machine declares, or an `unproven-1811` the reviewer accepts. A renamed
//! arm reprints as a new `unproven-1811` pair and loses its reason, so
//! re-attach the `invariant:` reasons of renamed arms. The contract checks
//! only that an `invariant:<name>` is declared, not that it separates the
//! pair; that argument belongs to the review.
//!
//! At introduction (base `d7916d84`) there were 1,045 groups with more than
//! one arm in 15 of the 17 canonical machines, and 10,548 arm pairs, of which
//! the rules refute 10,066. The 482 unrefuted pairs, all `unproven-1811`,
//! are: MeerkatMachine 264, MobMachine 92, WorkGraphLifecycleMachine 59,
//! DetachedJobMachine 20, ApprovalLifecycleMachine 18,
//! SessionDocumentMachine 15, RuntimeDeliveryMachine 8 and AuthMachine 6.
//! The 20 DetachedJobMachine pairs (terminal against notification delivery
//! arms of `MarkDeliveryApplied`) are now `invariant:` entries: every
//! notification is sequenced before the terminal delivery
//! (`terminal_delivery_follows_every_notification`), so the arms' sequence
//! checks never both hold, even for a notification named like the terminal
//! delivery. The RuntimeDeliveryMachine pair that #1811 names,
//! `ObserveAlreadyRefusedDelivery` against `SettleRefusedDeliveryAtCursor`,
//! is an `invariant:` entry too: a refused delivery is at or below the applied
//! cursor (`refused_deliveries_are_committed_and_passed`), and settling needs
//! a delivery above it.
//!
//! Follow-up (0.9.4): triage the `unproven-1811` entries (a checker gap, an
//! undeclared invariant, or a real overlap), have TLC prove exclusivity for
//! the groups that remain (`NoAmbiguousTransition` invariants), and assert
//! exclusivity in the generated dispatch chain in debug builds.

use std::collections::BTreeSet;

use meerkat_machine_schema::identity::TransitionId;
use meerkat_machine_schema::{
    Expr, HelperSchema, MachineSchema, TransitionSchema, TriggerMatch, canonical_machine_schemas,
};

const UNPROVEN: &str = "unproven-1811";
const INVARIANT_PREFIX: &str = "invariant:";
const MAX_INLINE_DEPTH: usize = 8;

/// One same-trigger arm pair, with its arms in name order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ArmPair {
    machine: String,
    phase: String,
    trigger: String,
    first: String,
    second: String,
}

impl std::fmt::Display for ArmPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} {} {} {}",
            self.machine, self.phase, self.trigger, self.first, self.second
        )
    }
}

fn trigger_name(transition: &TransitionSchema) -> String {
    match &transition.on {
        TriggerMatch::Input { variant, .. } => format!("input:{}", variant.as_str()),
        TriggerMatch::Signal { variant, .. } => format!("signal:{}", variant.as_str()),
    }
}

/// Structural refutation of same-trigger arm pairs in one machine.
struct Refuter<'a> {
    helpers: &'a [HelperSchema],
}

impl Refuter<'_> {
    fn inline(&self, helper: &str, args: &[Expr]) -> Option<Expr> {
        let schema = self.helpers.iter().find(|schema| schema.name == helper)?;
        (schema.params.len() == args.len()).then(|| {
            let params: Vec<(String, Expr)> = schema
                .params
                .iter()
                .zip(args)
                .map(|(param, arg)| (param.name.as_str().to_owned(), arg.clone()))
                .collect();
            substitute(&schema.body, &params)
        })
    }

    /// `expr` with boolean equalities folded, helpers inlined and option
    /// views rewritten.
    fn normalize(&self, expr: &Expr, depth: usize) -> Expr {
        if let Expr::Eq(left, right) | Expr::Neq(left, right) = expr {
            let negated = matches!(expr, Expr::Neq(..));
            let rewritten = option_view(left)
                .and_then(|view| self.option_view_eq(view, right, negated, depth))
                .or_else(|| {
                    option_view(right)
                        .and_then(|view| self.option_view_eq(view, left, negated, depth))
                });
            if let Some(rewritten) = rewritten {
                return rewritten;
            }
        }
        match expr {
            Expr::Call { helper, args } if depth < MAX_INLINE_DEPTH => {
                match self.inline(helper, args) {
                    Some(inlined) => self.normalize(&inlined, depth + 1),
                    None => expr.clone(),
                }
            }
            Expr::Eq(left, right) => match (&**left, &**right) {
                (other, Expr::Bool(true)) | (Expr::Bool(true), other) => {
                    self.normalize(other, depth)
                }
                (other, Expr::Bool(false)) | (Expr::Bool(false), other) => {
                    negate(self.normalize(other, depth))
                }
                _ => expr.clone(),
            },
            Expr::Neq(left, right) => match (&**left, &**right) {
                (other, Expr::Bool(false)) | (Expr::Bool(false), other) => {
                    self.normalize(other, depth)
                }
                (other, Expr::Bool(true)) | (Expr::Bool(true), other) => {
                    negate(self.normalize(other, depth))
                }
                _ => expr.clone(),
            },
            Expr::Not(inner) => negate(self.normalize(inner, depth)),
            other => other.clone(),
        }
    }

    fn option_view_eq(
        &self,
        (map, key): (&Expr, &Expr),
        other: &Expr,
        negated: bool,
        depth: usize,
    ) -> Option<Expr> {
        let contains = Expr::MapContainsKey {
            map: Box::new(map.clone()),
            key: Box::new(key.clone()),
        };
        let stored = Expr::MapGet {
            map: Box::new(map.clone()),
            key: Box::new(key.clone()),
        };
        match (other, negated) {
            (Expr::None, false) => Some(negate(contains)),
            (Expr::None, true) => Some(contains),
            (Expr::Some(value), false) => Some(Expr::And(vec![
                contains,
                Expr::Eq(Box::new(stored), Box::new(self.normalize(value, depth))),
            ])),
            (Expr::Some(value), true) => Some(Expr::Or(vec![
                negate(contains),
                Expr::Neq(Box::new(stored), Box::new(self.normalize(value, depth))),
            ])),
            _ => None,
        }
    }

    fn conjuncts(&self, expr: &Expr, out: &mut Vec<Expr>) {
        if let Expr::And(terms) = expr {
            for term in terms {
                self.conjuncts(term, out);
            }
            return;
        }
        match self.normalize(expr, 0) {
            Expr::And(terms) => {
                for term in &terms {
                    self.conjuncts(term, out);
                }
            }
            fact => out.push(fact),
        }
    }

    /// The conjuncts of `expr` and the facts their strict reads imply.
    fn facts(&self, expr: &Expr) -> Vec<Expr> {
        let mut facts = Vec::new();
        self.conjuncts(expr, &mut facts);
        let mut implied = Vec::new();
        for fact in &facts {
            strict_read_keys(fact, &mut implied);
        }
        for fact in implied {
            if !facts.contains(&fact) {
                facts.push(fact);
            }
        }
        facts
    }

    fn arm_facts(&self, arm: &TransitionSchema) -> Vec<Expr> {
        let mut facts = Vec::new();
        for guard in &arm.guards {
            for fact in self.facts(&guard.expr) {
                if !facts.contains(&fact) {
                    facts.push(fact);
                }
            }
        }
        facts
    }

    /// Whether no state and payload satisfy both fact sets.
    fn refutes(&self, a: &[Expr], b: &[Expr]) -> bool {
        if a.iter().any(|x| b.iter().any(|y| contradict(x, y))) {
            return true;
        }
        self.split_refutes(a, b) || self.split_refutes(b, a)
    }

    /// Some disjunction of `facts` whose every case is refuted by `other`.
    fn split_refutes(&self, facts: &[Expr], other: &[Expr]) -> bool {
        facts.iter().any(|fact| match fact {
            Expr::Or(cases) => cases
                .iter()
                .all(|case| self.refutes(&self.facts(case), other)),
            _ => false,
        })
    }
}

fn negate(expr: Expr) -> Expr {
    match expr {
        Expr::Not(inner) => *inner,
        other => Expr::Not(Box::new(other)),
    }
}

/// The `(map, key)` of the lowered `m.get_cloned(k)`:
/// `if m.contains_key(k) { Some(m[k]) } else { None }`.
fn option_view(expr: &Expr) -> Option<(&Expr, &Expr)> {
    let Expr::IfElse {
        condition,
        then_expr,
        else_expr,
    } = expr
    else {
        return None;
    };
    let (Expr::MapContainsKey { map, key }, Expr::Some(inner), Expr::None) =
        (&**condition, &**then_expr, &**else_expr)
    else {
        return None;
    };
    match &**inner {
        Expr::MapGet {
            map: stored_map,
            key: stored_key,
        } if stored_map == map && stored_key == key => Some((map, key)),
        _ => None,
    }
}

/// Keys that strict map reads in `expr` require: each `MapValue` evaluated
/// whenever `expr` is. Lazy contexts (`||`, conditionals, negation,
/// quantifiers, helper calls) imply nothing.
fn strict_read_keys(expr: &Expr, out: &mut Vec<Expr>) {
    match expr {
        Expr::MapValue { map, key } => {
            out.push(Expr::MapContainsKey {
                map: map.clone(),
                key: key.clone(),
            });
            strict_read_keys(map, out);
            strict_read_keys(key, out);
        }
        Expr::And(terms) => terms.iter().for_each(|term| strict_read_keys(term, out)),
        Expr::Eq(a, b)
        | Expr::Neq(a, b)
        | Expr::Add(a, b)
        | Expr::Sub(a, b)
        | Expr::Gt(a, b)
        | Expr::Gte(a, b)
        | Expr::Lt(a, b)
        | Expr::Lte(a, b) => {
            strict_read_keys(a, out);
            strict_read_keys(b, out);
        }
        Expr::Contains {
            collection: a,
            value: b,
        }
        | Expr::Count {
            collection: a,
            value: b,
        }
        | Expr::MapContainsKey { map: a, key: b }
        | Expr::MapGet { map: a, key: b }
        | Expr::SeqStartsWith { seq: a, prefix: b } => {
            strict_read_keys(a, out);
            strict_read_keys(b, out);
        }
        Expr::FieldAccess { base: inner, .. }
        | Expr::Some(inner)
        | Expr::SeqElements(inner)
        | Expr::Len(inner)
        | Expr::Head(inner)
        | Expr::MapKeys(inner)
        | Expr::EnumVariantIs { value: inner, .. }
        | Expr::EnumStringSetPayload { value: inner, .. } => strict_read_keys(inner, out),
        Expr::Bool(_)
        | Expr::U64(_)
        | Expr::U64Max
        | Expr::String(_)
        | Expr::NamedVariant { .. }
        | Expr::EmptySet
        | Expr::EmptyMap
        | Expr::SeqLiteral(_)
        | Expr::CurrentPhase
        | Expr::Phase(_)
        | Expr::Field(_)
        | Expr::Binding(_)
        | Expr::Variant(_)
        | Expr::None
        | Expr::IfElse { .. }
        | Expr::Not(_)
        | Expr::Or(_)
        | Expr::MapLiteral(_)
        | Expr::Call { .. }
        | Expr::Quantified { .. } => {}
    }
}

/// `expr` with each named parameter replaced by its argument, never inside
/// a quantifier that rebinds the name.
fn substitute(expr: &Expr, params: &[(String, Expr)]) -> Expr {
    let sub = |inner: &Expr| Box::new(substitute(inner, params));
    let all = |items: &[Expr]| items.iter().map(|item| substitute(item, params)).collect();
    match expr {
        Expr::Binding(name) => params
            .iter()
            .find(|(param, _)| param == name)
            .map_or_else(|| expr.clone(), |(_, arg)| arg.clone()),
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
        | Expr::Variant(_)
        | Expr::None => expr.clone(),
        Expr::FieldAccess { base, field } => Expr::FieldAccess {
            base: sub(base),
            field: field.clone(),
        },
        Expr::EnumVariantIs {
            value,
            enum_name,
            variant,
        } => Expr::EnumVariantIs {
            value: sub(value),
            enum_name: enum_name.clone(),
            variant: variant.clone(),
        },
        Expr::EnumStringSetPayload {
            value,
            enum_name,
            variant,
            field,
        } => Expr::EnumStringSetPayload {
            value: sub(value),
            enum_name: enum_name.clone(),
            variant: variant.clone(),
            field: field.clone(),
        },
        Expr::SeqLiteral(items) => Expr::SeqLiteral(all(items)),
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => Expr::IfElse {
            condition: sub(condition),
            then_expr: sub(then_expr),
            else_expr: sub(else_expr),
        },
        Expr::Not(inner) => Expr::Not(sub(inner)),
        Expr::And(terms) => Expr::And(all(terms)),
        Expr::Or(terms) => Expr::Or(all(terms)),
        Expr::Eq(a, b) => Expr::Eq(sub(a), sub(b)),
        Expr::Neq(a, b) => Expr::Neq(sub(a), sub(b)),
        Expr::Add(a, b) => Expr::Add(sub(a), sub(b)),
        Expr::Sub(a, b) => Expr::Sub(sub(a), sub(b)),
        Expr::Gt(a, b) => Expr::Gt(sub(a), sub(b)),
        Expr::Gte(a, b) => Expr::Gte(sub(a), sub(b)),
        Expr::Lt(a, b) => Expr::Lt(sub(a), sub(b)),
        Expr::Lte(a, b) => Expr::Lte(sub(a), sub(b)),
        Expr::Contains { collection, value } => Expr::Contains {
            collection: sub(collection),
            value: sub(value),
        },
        Expr::MapContainsKey { map, key } => Expr::MapContainsKey {
            map: sub(map),
            key: sub(key),
        },
        Expr::SeqStartsWith { seq, prefix } => Expr::SeqStartsWith {
            seq: sub(seq),
            prefix: sub(prefix),
        },
        Expr::SeqElements(inner) => Expr::SeqElements(sub(inner)),
        Expr::Len(inner) => Expr::Len(sub(inner)),
        Expr::Count { collection, value } => Expr::Count {
            collection: sub(collection),
            value: sub(value),
        },
        Expr::Head(inner) => Expr::Head(sub(inner)),
        Expr::MapKeys(inner) => Expr::MapKeys(sub(inner)),
        Expr::MapGet { map, key } => Expr::MapGet {
            map: sub(map),
            key: sub(key),
        },
        Expr::MapValue { map, key } => Expr::MapValue {
            map: sub(map),
            key: sub(key),
        },
        Expr::MapLiteral(entries) => Expr::MapLiteral(
            entries
                .iter()
                .map(|(key, value)| (substitute(key, params), substitute(value, params)))
                .collect(),
        ),
        Expr::Some(inner) => Expr::Some(sub(inner)),
        Expr::Call { helper, args } => Expr::Call {
            helper: helper.clone(),
            args: all(args),
        },
        Expr::Quantified {
            quantifier,
            binding,
            over,
            body,
        } => {
            let unshadowed: Vec<(String, Expr)> = params
                .iter()
                .filter(|(param, _)| param != binding)
                .cloned()
                .collect();
            Expr::Quantified {
                quantifier: quantifier.clone(),
                binding: binding.clone(),
                over: sub(over),
                body: Box::new(substitute(body, &unshadowed)),
            }
        }
    }
}

/// A literal of a kind whose distinct spellings are distinct values.
fn literal_kind(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Bool(_) => Some("bool".into()),
        Expr::U64(_) => Some("u64".into()),
        Expr::String(_) => Some("string".into()),
        Expr::NamedVariant { enum_name, .. } => Some(format!("enum:{}", enum_name.as_str())),
        Expr::Phase(_) => Some("phase".into()),
        Expr::None => Some("option".into()),
        Expr::Some(inner) => literal_kind(inner).map(|kind| format!("option:{kind}")),
        _ => None,
    }
}

/// Whether two literals are certainly different values.
fn distinct_literals(a: &Expr, b: &Expr) -> bool {
    match (a, b) {
        (Expr::None, Expr::Some(_)) | (Expr::Some(_), Expr::None) => true,
        (Expr::Some(a), Expr::Some(b)) => distinct_literals(a, b),
        _ => match (literal_kind(a), literal_kind(b)) {
            (Some(kind_a), Some(kind_b)) => kind_a == kind_b && a != b,
            _ => false,
        },
    }
}

/// `(subject, literal)` of an equality with exactly one literal side.
fn equals_literal(expr: &Expr) -> Option<(&Expr, &Expr)> {
    let Expr::Eq(left, right) = expr else {
        return None;
    };
    match (literal_kind(left), literal_kind(right)) {
        (None, Some(_)) => Some((left, right)),
        (Some(_), None) => Some((right, left)),
        _ => None,
    }
}

/// `(subject, is_none)` of `subject == None`, `subject == Some(_)` or
/// `subject != None`.
fn option_shape(expr: &Expr) -> Option<(&Expr, bool)> {
    let (subject, other, negated) = match expr {
        Expr::Eq(left, right) => match (&**left, &**right) {
            (Expr::None | Expr::Some(_), subject) => (subject, &**left, false),
            (subject, other) => (subject, other, false),
        },
        Expr::Neq(left, right) => match (&**left, &**right) {
            (Expr::None, subject) => (subject, &**left, true),
            (subject, other) => (subject, other, true),
        },
        _ => return None,
    };
    match (other, negated) {
        (Expr::None, false) => Some((subject, true)),
        (Expr::Some(_), false) | (Expr::None, true) => Some((subject, false)),
        _ => None,
    }
}

/// `(relation, lhs, rhs)` with `>`/`>=` turned around into `<`/`<=`.
fn relation(expr: &Expr) -> Option<(&'static str, &Expr, &Expr)> {
    match expr {
        Expr::Lt(a, b) => Some(("lt", a, b)),
        Expr::Lte(a, b) => Some(("le", a, b)),
        Expr::Gt(a, b) => Some(("lt", b, a)),
        Expr::Gte(a, b) => Some(("le", b, a)),
        Expr::Eq(a, b) => Some(("eq", a, b)),
        Expr::Neq(a, b) => Some(("ne", a, b)),
        _ => None,
    }
}

/// Whether two conjuncts cannot both hold.
fn contradict(a: &Expr, b: &Expr) -> bool {
    if matches!(a, Expr::Bool(false)) || matches!(b, Expr::Bool(false)) {
        return true;
    }
    if matches!(a, Expr::Not(inner) if **inner == *b)
        || matches!(b, Expr::Not(inner) if **inner == *a)
    {
        return true;
    }
    if let (Some((subject_a, literal_a)), Some((subject_b, literal_b))) =
        (equals_literal(a), equals_literal(b))
        && subject_a == subject_b
        && distinct_literals(literal_a, literal_b)
    {
        return true;
    }
    if let (Some((subject_a, none_a)), Some((subject_b, none_b))) =
        (option_shape(a), option_shape(b))
        && subject_a == subject_b
        && none_a != none_b
    {
        return true;
    }
    if let (
        Expr::EnumVariantIs {
            value: value_a,
            enum_name: enum_a,
            variant: variant_a,
        },
        Expr::EnumVariantIs {
            value: value_b,
            enum_name: enum_b,
            variant: variant_b,
        },
    ) = (a, b)
        && value_a == value_b
        && enum_a == enum_b
        && variant_a != variant_b
    {
        return true;
    }
    if let (Some((op_a, lhs_a, rhs_a)), Some((op_b, lhs_b, rhs_b))) = (relation(a), relation(b)) {
        let same = lhs_a == lhs_b && rhs_a == rhs_b;
        let turned = lhs_a == rhs_b && rhs_a == lhs_b;
        let ops = |x: &str, y: &str| (op_a == x && op_b == y) || (op_a == y && op_b == x);
        if (same || turned) && (ops("eq", "ne") || ops("lt", "eq")) {
            return true;
        }
        // a < b with b <= a or b < a.
        if turned && (ops("lt", "le") || (op_a == "lt" && op_b == "lt")) {
            return true;
        }
    }
    false
}

/// The same-trigger arm pairs of the canonical machines.
struct Census {
    /// Groups (machine, phase, trigger) with more than one arm.
    groups: usize,
    pairs: usize,
    /// The pairs the rules cannot refute.
    unrefuted: BTreeSet<ArmPair>,
}

fn census(machines: &[MachineSchema]) -> Census {
    let mut unrefuted = BTreeSet::new();
    let mut pairs = 0;
    let mut multi_arm_groups = 0;
    for machine in machines {
        let refuter = Refuter {
            helpers: &machine.helpers,
        };
        let facts: Vec<Vec<Expr>> = machine
            .transitions
            .iter()
            .map(|arm| refuter.arm_facts(arm))
            .collect();
        let mut groups: std::collections::BTreeMap<(String, String), Vec<usize>> =
            std::collections::BTreeMap::new();
        for (index, arm) in machine.transitions.iter().enumerate() {
            for phase in &arm.from {
                groups
                    .entry((phase.as_str().to_owned(), trigger_name(arm)))
                    .or_default()
                    .push(index);
            }
        }
        for ((phase, trigger), arms) in groups {
            if arms.len() > 1 {
                multi_arm_groups += 1;
            }
            for (position, &a) in arms.iter().enumerate() {
                for &b in &arms[position + 1..] {
                    pairs += 1;
                    if refuter.refutes(&facts[a], &facts[b]) {
                        continue;
                    }
                    let mut names = [
                        machine.transitions[a].name.as_str().to_owned(),
                        machine.transitions[b].name.as_str().to_owned(),
                    ];
                    names.sort();
                    let [first, second] = names;
                    unrefuted.insert(ArmPair {
                        machine: machine.machine.as_str().to_owned(),
                        phase: phase.clone(),
                        trigger: trigger.clone(),
                        first,
                        second,
                    });
                }
            }
        }
    }
    Census {
        groups: multi_arm_groups,
        pairs,
        unrefuted,
    }
}

/// The baseline entries: each pair with its reason.
fn parse_baseline(baseline: &str) -> Vec<(ArmPair, String)> {
    baseline
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [machine, phase, trigger, first, second, reason] = fields[..] else {
                panic!("baseline line must have 6 fields: {line}");
            };
            assert!(
                first < second,
                "baseline arms must be in name order: {line}"
            );
            (
                ArmPair {
                    machine: machine.to_owned(),
                    phase: phase.to_owned(),
                    trigger: trigger.to_owned(),
                    first: first.to_owned(),
                    second: second.to_owned(),
                },
                reason.to_owned(),
            )
        })
        .collect()
}

/// What the contract refuses.
#[derive(Debug, Default, PartialEq, Eq)]
struct Drift {
    /// Unrefuted pairs the baseline does not list.
    new: Vec<String>,
    /// Baseline pairs that are refutable now, or whose arms are gone.
    stale: Vec<String>,
    /// Baseline reasons naming an invariant the machine does not declare,
    /// or no known reason.
    unjustified: Vec<String>,
}

fn exclusivity_drift(machines: &[MachineSchema], baseline: &str) -> Drift {
    let unrefuted = census(machines).unrefuted;
    let entries = parse_baseline(baseline);
    let listed: BTreeSet<&ArmPair> = entries.iter().map(|(pair, _)| pair).collect();
    let mut drift = Drift {
        new: unrefuted
            .iter()
            .filter(|pair| !listed.contains(pair))
            .map(ToString::to_string)
            .collect(),
        ..Drift::default()
    };
    for (pair, reason) in &entries {
        if !unrefuted.contains(pair) {
            drift.stale.push(pair.to_string());
        }
        let justified = reason == UNPROVEN
            || reason.strip_prefix(INVARIANT_PREFIX).is_some_and(|name| {
                machines
                    .iter()
                    .find(|machine| machine.machine.as_str() == pair.machine)
                    .is_some_and(|machine| {
                        machine
                            .invariants
                            .iter()
                            .any(|invariant| invariant.name == name)
                    })
            });
        if !justified {
            drift.unjustified.push(format!("{pair} {reason}"));
        }
    }
    drift
}

#[test]
fn same_trigger_arms_are_exclusive_or_baselined() {
    let machines = canonical_machine_schemas();
    let census = census(&machines);
    assert!(
        census.groups > 0 && census.pairs > census.unrefuted.len(),
        "positive control: there are groups, and the rules refute some of their {} pairs",
        census.pairs
    );
    assert_eq!(exclusivity_drift(&machines, BASELINE), Drift::default());
}

/// Prints the current unrefuted set in baseline form (reason
/// `unproven-1811`), for a reviewed baseline update.
#[test]
#[ignore = "prints the baseline for a reviewed update"]
fn print_exclusivity_baseline() {
    let machines = canonical_machine_schemas();
    let census = census(&machines);
    let reasons: std::collections::BTreeMap<ArmPair, String> =
        parse_baseline(BASELINE).into_iter().collect();
    // Under `--nocapture` libtest has already printed `test <name> ... `
    // without a newline: start the pairs on a line of their own.
    println!();
    eprintln!(
        "# {} groups with more than one arm, {} pairs, {} unrefuted",
        census.groups,
        census.pairs,
        census.unrefuted.len()
    );
    for pair in &census.unrefuted {
        let reason = reasons.get(pair).map_or(UNPROVEN, String::as_str);
        println!("{pair} {reason}");
    }
}

fn machine<'a>(machines: &'a mut [MachineSchema], name: &str) -> &'a mut MachineSchema {
    machines
        .iter_mut()
        .find(|machine| machine.machine.as_str() == name)
        .unwrap_or_else(|| panic!("{name}"))
}

fn arm<'a>(machine: &'a mut MachineSchema, name: &str) -> &'a mut TransitionSchema {
    machine
        .transitions
        .iter_mut()
        .find(|transition| transition.name.as_str() == name)
        .unwrap_or_else(|| panic!("{name}"))
}

fn mentions_field(expr: &Expr, field: &str) -> bool {
    format!("{expr:?}").contains(&format!("FieldId(\"{field}\")"))
}

/// Mutant (a): a revoke that no longer requires the record to be
/// unrevoked overlaps the already-revoked arm.
#[test]
fn an_arm_without_its_separating_conjunct_is_refused() {
    let mut machines = canonical_machine_schemas();
    arm(machine(&mut machines, "GrantAuthorityMachine"), "RevokeNew")
        .guards
        .retain(|guard| !mentions_field(&guard.expr, "revoked"));
    assert_eq!(
        exclusivity_drift(&machines, BASELINE).new,
        ["GrantAuthorityMachine Active input:Revoke RevokeAlready RevokeNew"]
    );
}

/// Mutant (b): a second arm with an existing arm's guards is refused.
#[test]
fn a_duplicated_arm_is_refused() {
    let mut machines = canonical_machine_schemas();
    let grant = machine(&mut machines, "GrantAuthorityMachine");
    let mut copy = arm(grant, "RevokeAlready").clone();
    copy.name = TransitionId::parse("RevokeAlreadyCopy").expect("transition id");
    grant.transitions.push(copy);
    assert_eq!(
        exclusivity_drift(&machines, BASELINE).new,
        ["GrantAuthorityMachine Active input:Revoke RevokeAlready RevokeAlreadyCopy"]
    );
}

/// Mutant (c): an unrefuted pair dropped from the baseline is refused.
#[test]
fn a_dropped_baseline_entry_is_refused() {
    let machines = canonical_machine_schemas();
    let dropped = BASELINE
        .lines()
        .find(|line| !line.trim().is_empty())
        .expect("a baseline entry");
    let without = BASELINE.replacen(dropped, "", 1);
    let pair = dropped.rsplit_once(' ').expect("reason").0;
    assert_eq!(exclusivity_drift(&machines, &without).new, [pair]);
}

/// Mutant (d): a baseline reason naming an invariant the machine does not
/// declare is refused.
#[test]
fn an_undeclared_baseline_invariant_is_refused() {
    let machines = canonical_machine_schemas();
    let entry = BASELINE
        .lines()
        .find(|line| !line.trim().is_empty())
        .expect("a baseline entry");
    let pair = entry.rsplit_once(' ').expect("reason").0;
    let rewritten = BASELINE.replacen(
        entry,
        &format!("{pair} {INVARIANT_PREFIX}no_such_invariant"),
        1,
    );
    let drift = exclusivity_drift(&machines, &rewritten);
    assert_eq!(
        drift.unjustified,
        [format!("{pair} {INVARIANT_PREFIX}no_such_invariant")]
    );
    assert!(drift.new.is_empty() && drift.stale.is_empty());
}

/// A baseline entry for a refutable pair is stale and refused.
#[test]
fn a_stale_baseline_entry_is_refused() {
    let machines = canonical_machine_schemas();
    let stale = "GrantAuthorityMachine Active input:Revoke RevokeAlready RevokeNew";
    let with_stale = format!("{BASELINE}{stale} {UNPROVEN}\n");
    assert_eq!(exclusivity_drift(&machines, &with_stale).stale, [stale]);
}

/// Unrefuted same-trigger arm pairs, one per line:
/// `machine phase trigger first second reason`.
const BASELINE: &str = r"
ApprovalLifecycleMachine Ready input:DecideConsent DecideConsentApprove DecideConsentRejectedMissing unproven-1811
ApprovalLifecycleMachine Ready input:DecideConsent DecideConsentDeny DecideConsentRejectedMissing unproven-1811
ApprovalLifecycleMachine Ready input:DecideConsent DecideConsentRejectedAlreadyDecided DecideConsentRejectedMissing unproven-1811
ApprovalLifecycleMachine Ready input:DecideConsent DecideConsentRejectedAuthenticationRequired DecideConsentRejectedMissing unproven-1811
ApprovalLifecycleMachine Ready input:DecideConsent DecideConsentRejectedExpired DecideConsentRejectedMissing unproven-1811
ApprovalLifecycleMachine Ready input:DecideConsent DecideConsentRejectedMissing DecideConsentRejectedNotAllowed unproven-1811
ApprovalLifecycleMachine Ready input:ReconcileApproval ReconcileAppliedApproved ReconcileRejectedInvalidRecord unproven-1811
ApprovalLifecycleMachine Ready input:ReconcileApproval ReconcileAppliedCancelled ReconcileRejectedInvalidRecord unproven-1811
ApprovalLifecycleMachine Ready input:ReconcileApproval ReconcileAppliedDenied ReconcileRejectedInvalidRecord unproven-1811
ApprovalLifecycleMachine Ready input:ReconcileApproval ReconcileAppliedExpired ReconcileRejectedInvalidRecord unproven-1811
ApprovalLifecycleMachine Ready input:ReconcileApproval ReconcileAppliedPending ReconcileRejectedInvalidRecord unproven-1811
ApprovalLifecycleMachine Ready input:RestoreApproval RestoreApproved RestoreRejectedInvalidRecord unproven-1811
ApprovalLifecycleMachine Ready input:RestoreApproval RestoreCancelled RestoreRejectedInvalidRecord unproven-1811
ApprovalLifecycleMachine Ready input:RestoreApproval RestoreDenied RestoreRejectedInvalidRecord unproven-1811
ApprovalLifecycleMachine Ready input:RestoreApproval RestoreExpired RestoreRejectedInvalidRecord unproven-1811
ApprovalLifecycleMachine Ready input:RestoreApproval RestorePending RestoreRejectedInvalidRecord unproven-1811
ApprovalLifecycleMachine Ready input:WithdrawConsent WithdrawConsentPending WithdrawConsentRejectedMissing unproven-1811
ApprovalLifecycleMachine Ready input:WithdrawConsent WithdrawConsentRejectedMissing WithdrawConsentSettled unproven-1811
AuthMachine Expiring input:ObserveCredentialFreshness ObserveCredentialFreshnessExpiredFromExpiring ObserveCredentialFreshnessExpiring unproven-1811
AuthMachine Valid input:ObserveCredentialFreshness ObserveCredentialFreshnessExpiredFromValid ObserveCredentialFreshnessExpiringFromValid unproven-1811
AuthMachine Valid input:ObserveCredentialFreshness ObserveCredentialFreshnessExpiredFromValid ObserveCredentialFreshnessValid unproven-1811
AuthMachine Valid input:ObserveCredentialFreshness ObserveCredentialFreshnessExpiringFromValid ObserveCredentialFreshnessValid unproven-1811
AuthMachine Valid input:ResolveOAuthLoginCredentialDisposition ResolveOAuthLoginCredentialDispositionRefreshDisallowedValidValid ResolveOAuthLoginCredentialDispositionUseCachedValid unproven-1811
AuthMachine Valid input:ResolveOAuthLoginCredentialDisposition ResolveOAuthLoginCredentialDispositionRefreshValidValid ResolveOAuthLoginCredentialDispositionUseCachedValid unproven-1811
DetachedJobMachine Cancelled input:MarkDeliveryApplied ApplyCancelledDelivery ApplyCancelledNotificationDelivery invariant:terminal_delivery_follows_every_notification
DetachedJobMachine Cancelled input:MarkDeliveryApplied ApplyCancelledDelivery ObserveCancelledNotificationDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine Cancelled input:MarkDeliveryApplied ApplyCancelledNotificationDelivery ObserveCancelledDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine Cancelled input:MarkDeliveryApplied ObserveCancelledDeliveryAlreadyApplied ObserveCancelledNotificationDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine Failed input:MarkDeliveryApplied ApplyFailedDelivery ApplyFailedNotificationDelivery invariant:terminal_delivery_follows_every_notification
DetachedJobMachine Failed input:MarkDeliveryApplied ApplyFailedDelivery ObserveFailedNotificationDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine Failed input:MarkDeliveryApplied ApplyFailedNotificationDelivery ObserveFailedDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine Failed input:MarkDeliveryApplied ObserveFailedDeliveryAlreadyApplied ObserveFailedNotificationDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine NeedsAttention input:MarkDeliveryApplied ApplyNeedsAttentionDelivery ApplyNeedsAttentionNotificationDelivery invariant:terminal_delivery_follows_every_notification
DetachedJobMachine NeedsAttention input:MarkDeliveryApplied ApplyNeedsAttentionDelivery ObserveNeedsAttentionNotificationDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine NeedsAttention input:MarkDeliveryApplied ApplyNeedsAttentionNotificationDelivery ObserveNeedsAttentionDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine NeedsAttention input:MarkDeliveryApplied ObserveNeedsAttentionDeliveryAlreadyApplied ObserveNeedsAttentionNotificationDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine Succeeded input:MarkDeliveryApplied ApplySucceededDelivery ApplySucceededNotificationDelivery invariant:terminal_delivery_follows_every_notification
DetachedJobMachine Succeeded input:MarkDeliveryApplied ApplySucceededDelivery ObserveSucceededNotificationDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine Succeeded input:MarkDeliveryApplied ApplySucceededNotificationDelivery ObserveSucceededDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine Succeeded input:MarkDeliveryApplied ObserveSucceededDeliveryAlreadyApplied ObserveSucceededNotificationDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine WorkerLost input:MarkDeliveryApplied ApplyWorkerLostDelivery ApplyWorkerLostNotificationDelivery invariant:terminal_delivery_follows_every_notification
DetachedJobMachine WorkerLost input:MarkDeliveryApplied ApplyWorkerLostDelivery ObserveWorkerLostNotificationDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine WorkerLost input:MarkDeliveryApplied ApplyWorkerLostNotificationDelivery ObserveWorkerLostDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
DetachedJobMachine WorkerLost input:MarkDeliveryApplied ObserveWorkerLostDeliveryAlreadyApplied ObserveWorkerLostNotificationDeliveryAlreadyApplied invariant:terminal_delivery_follows_every_notification
MeerkatMachine Attached input:AdmitModelRoutingAssistantTurn AdmitPendingFiniteSwitchTurnAttached DecrementFiniteSwitchTurnAttached unproven-1811
MeerkatMachine Attached input:AdmitModelRoutingAssistantTurn AdmitPendingFiniteSwitchTurnAttached RestoreConsumedFiniteSwitchTurnAttached unproven-1811
MeerkatMachine Attached input:AdvanceLiveAssistantPlaybackSegment AdvanceLiveAssistantPlaybackSegmentAttached ReplayLiveAssistantPlaybackSegmentAttached unproven-1811
MeerkatMachine Attached input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendAttached AuthorizeLiveContextAppendSupersededAttached unproven-1811
MeerkatMachine Attached input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByCloseAttached AuthorizeLiveContextAppendDeliveredReplayAttached unproven-1811
MeerkatMachine Attached input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByConversationAttached AuthorizeLiveContextAppendDeferredByTurnAttached unproven-1811
MeerkatMachine Attached input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByConversationAttached AuthorizeLiveContextAppendDeliveredReplayAttached unproven-1811
MeerkatMachine Attached input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByConversationAttached AuthorizeLiveContextAppendSupersededAttached unproven-1811
MeerkatMachine Attached input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByRecoveryAttached AuthorizeLiveContextAppendDeliveredReplayAttached unproven-1811
MeerkatMachine Attached input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByTurnAttached AuthorizeLiveContextAppendDeliveredReplayAttached unproven-1811
MeerkatMachine Attached input:BeginImageOperation BeginImageOperationApprovalDeniedAttached BeginImageOperationScopedConflictAttached unproven-1811
MeerkatMachine Attached input:BeginImageOperation BeginImageOperationApprovalUnavailableAttached BeginImageOperationScopedConflictAttached unproven-1811
MeerkatMachine Attached input:CancelLiveRecoveryObligation CancelLiveContextRecoveryObligationAttached CancelLiveDelegationResultRecoveryObligationAttached unproven-1811
MeerkatMachine Attached input:ClaimModelRoutingHandoff ClaimModelRoutingHandoffAlreadyClaimedAttached ClaimModelRoutingHandoffAlreadyRealizedAttached unproven-1811
MeerkatMachine Attached input:ClaimModelRoutingHandoff ClaimModelRoutingHandoffAlreadyClaimedAttached ClaimModelRoutingHandoffImportedAttached unproven-1811
MeerkatMachine Attached input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalCancelledAttached ClassifyImageOperationTerminalRefusedAttached unproven-1811
MeerkatMachine Attached input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalCancelledAttached ClassifyImageOperationTerminalSafetyFilteredAttached unproven-1811
MeerkatMachine Attached input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalCancelledAttached ClassifyImageOperationTerminalTimeoutAttached unproven-1811
MeerkatMachine Attached input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalRefusedAttached ClassifyImageOperationTerminalTimeoutAttached unproven-1811
MeerkatMachine Attached input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalSafetyFilteredAttached ClassifyImageOperationTerminalTimeoutAttached unproven-1811
MeerkatMachine Attached input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonAttached ClassifyStopInputDispositionRetainAttached unproven-1811
MeerkatMachine Attached input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonAttached ClassifyStopInputDispositionRetainResumableAttached unproven-1811
MeerkatMachine Attached input:ClassifyTerminalCompletionCorrelation ClassifyTerminalCompletionCorrelationAbandonedAttached ClassifyTerminalCompletionCorrelationRunAttached unproven-1811
MeerkatMachine Attached input:DenyModelRoutingHandoff DenyModelRoutingHandoffAlreadyDeniedAttached DenyModelRoutingHandoffPendingAttached unproven-1811
MeerkatMachine Attached input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedAttached ObserveSupervisorRotationNextPublishPendingAttached unproven-1811
MeerkatMachine Attached input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedAttached ObserveSupervisorRotationNotFoundAttached unproven-1811
MeerkatMachine Attached input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedAttached ObserveSupervisorRotationPreviousRevokePendingAttached unproven-1811
MeerkatMachine Attached input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingAttached ObserveSupervisorRotationNotFoundAttached unproven-1811
MeerkatMachine Attached input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingAttached ObserveSupervisorRotationRejectedAttached unproven-1811
MeerkatMachine Attached input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundAttached ObserveSupervisorRotationPreviousRevokePendingAttached unproven-1811
MeerkatMachine Attached input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundAttached ObserveSupervisorRotationRejectedAttached unproven-1811
MeerkatMachine Attached input:ObserveSupervisorRotation ObserveSupervisorRotationPreviousRevokePendingAttached ObserveSupervisorRotationRejectedAttached unproven-1811
MeerkatMachine Attached input:RealizeModelRoutingHandoff RealizeModelRoutingHandoffAlreadyRealizedAttached RealizeModelRoutingHandoffClaimedAttached unproven-1811
MeerkatMachine Attached input:ReleaseRunStarts ReleaseRunStartsLastAttached ReleaseRunStartsStillHeldAttached unproven-1811
MeerkatMachine Attached input:RequestFiniteSwitchTurn RequestFiniteSwitchTurnApprovalDeniedAttached RequestFiniteSwitchTurnScopedConflictAttached unproven-1811
MeerkatMachine Attached input:RequestFiniteSwitchTurn RequestFiniteSwitchTurnApprovalUnavailableAttached RequestFiniteSwitchTurnScopedConflictAttached unproven-1811
MeerkatMachine Attached input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionAcceptedAttached ResolveLiveWebrtcAnswerAdmissionChannelNotBoundAttached unproven-1811
MeerkatMachine Attached input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionChannelNotBoundAttached ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedAttached unproven-1811
MeerkatMachine Attached input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionChannelNotBoundAttached ResolveLiveWebrtcAnswerAdmissionTokenNotFoundAttached unproven-1811
MeerkatMachine Attached input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedAttached ResolveLiveWebrtcAnswerAdmissionTokenNotFoundAttached unproven-1811
MeerkatMachine Attached input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionAcceptedAttached ResolveLiveWebsocketTokenAdmissionChannelNotBoundAttached unproven-1811
MeerkatMachine Attached input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionChannelNotBoundAttached ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedAttached unproven-1811
MeerkatMachine Attached input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionChannelNotBoundAttached ResolveLiveWebsocketTokenAdmissionTokenNotFoundAttached unproven-1811
MeerkatMachine Attached input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedAttached ResolveLiveWebsocketTokenAdmissionTokenNotFoundAttached unproven-1811
MeerkatMachine Attached input:ResolveSupervisorAuthorizeAdmission ResolveSupervisorAuthorizeAdmissionIdempotentAckAttached ResolveSupervisorAuthorizeAdmissionProceedAttached unproven-1811
MeerkatMachine Attached input:ResolveSupervisorAuthorizeAdmission ResolveSupervisorAuthorizeAdmissionIdempotentAckAttached ResolveSupervisorAuthorizeAdmissionStaleSupervisorAttached unproven-1811
MeerkatMachine Attached input:RevokeLiveChannelCloseCustody RevokeLiveChannelCloseCustodyAttached RevokeLiveChannelCloseCustodyClosedReplayAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationAdoptCurrentAttached SubmitSupervisorRotationConflictAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationAdoptCurrentAttached SubmitSupervisorRotationNewAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationAdoptCurrentAttached SubmitSupervisorRotationPersistRejectedAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationConflictAttached SubmitSupervisorRotationExistingCompletedAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationConflictAttached SubmitSupervisorRotationExistingPendingAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationConflictAttached SubmitSupervisorRotationExistingRejectedAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationConflictAttached SubmitSupervisorRotationNewAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationConflictAttached SubmitSupervisorRotationPersistPreflightRejectedAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationConflictAttached SubmitSupervisorRotationPersistRejectedAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedAttached SubmitSupervisorRotationExistingPendingAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedAttached SubmitSupervisorRotationUnavailableAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationExistingPendingAttached SubmitSupervisorRotationExistingRejectedAttached unproven-1811
MeerkatMachine Attached input:SubmitSupervisorRotation SubmitSupervisorRotationExistingRejectedAttached SubmitSupervisorRotationUnavailableAttached unproven-1811
MeerkatMachine Attached input:SupersedeLiveInteraction SupersedeCompletedLiveInteractionDelegationWithCancellationAttached SupersedeLiveInteractionWithDelegationCancellationAttached unproven-1811
MeerkatMachine Attached input:SupersedeLiveInteraction SupersedeCompletedLiveInteractionDelegationWithCancellationAttached SupersedeLiveInteractionWithoutDelegationCancellationAttached unproven-1811
MeerkatMachine Attached input:SupersedeLiveInteraction SupersedeLiveInteractionWithDelegationCancellationAttached SupersedeLiveInteractionWithoutDelegationCancellationAttached unproven-1811
MeerkatMachine Destroyed input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedDestroyed ObserveSupervisorRotationNextPublishPendingDestroyed unproven-1811
MeerkatMachine Destroyed input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedDestroyed ObserveSupervisorRotationNotFoundDestroyed unproven-1811
MeerkatMachine Destroyed input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedDestroyed ObserveSupervisorRotationPreviousRevokePendingDestroyed unproven-1811
MeerkatMachine Destroyed input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingDestroyed ObserveSupervisorRotationNotFoundDestroyed unproven-1811
MeerkatMachine Destroyed input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingDestroyed ObserveSupervisorRotationRejectedDestroyed unproven-1811
MeerkatMachine Destroyed input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundDestroyed ObserveSupervisorRotationPreviousRevokePendingDestroyed unproven-1811
MeerkatMachine Destroyed input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundDestroyed ObserveSupervisorRotationRejectedDestroyed unproven-1811
MeerkatMachine Destroyed input:ObserveSupervisorRotation ObserveSupervisorRotationPreviousRevokePendingDestroyed ObserveSupervisorRotationRejectedDestroyed unproven-1811
MeerkatMachine Destroyed input:ReleaseRunStarts ReleaseRunStartsLastDestroyed ReleaseRunStartsStillHeldDestroyed unproven-1811
MeerkatMachine Destroyed input:ResolveSupervisorAuthorizeAdmission ResolveSupervisorAuthorizeAdmissionIdempotentAckDestroyed ResolveSupervisorAuthorizeAdmissionStaleSupervisorDestroyed unproven-1811
MeerkatMachine Destroyed input:SubmitSupervisorRotation SubmitSupervisorRotationConflictDestroyed SubmitSupervisorRotationExistingCompletedDestroyed unproven-1811
MeerkatMachine Destroyed input:SubmitSupervisorRotation SubmitSupervisorRotationConflictDestroyed SubmitSupervisorRotationExistingPendingDestroyed unproven-1811
MeerkatMachine Destroyed input:SubmitSupervisorRotation SubmitSupervisorRotationConflictDestroyed SubmitSupervisorRotationExistingRejectedDestroyed unproven-1811
MeerkatMachine Destroyed input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedDestroyed SubmitSupervisorRotationExistingPendingDestroyed unproven-1811
MeerkatMachine Destroyed input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedDestroyed SubmitSupervisorRotationUnavailableDestroyed unproven-1811
MeerkatMachine Destroyed input:SubmitSupervisorRotation SubmitSupervisorRotationExistingPendingDestroyed SubmitSupervisorRotationExistingRejectedDestroyed unproven-1811
MeerkatMachine Destroyed input:SubmitSupervisorRotation SubmitSupervisorRotationExistingRejectedDestroyed SubmitSupervisorRotationUnavailableDestroyed unproven-1811
MeerkatMachine Idle input:AdmitModelRoutingAssistantTurn AdmitPendingFiniteSwitchTurnIdle DecrementFiniteSwitchTurnIdle unproven-1811
MeerkatMachine Idle input:AdmitModelRoutingAssistantTurn AdmitPendingFiniteSwitchTurnIdle RestoreConsumedFiniteSwitchTurnIdle unproven-1811
MeerkatMachine Idle input:AdvanceLiveAssistantPlaybackSegment AdvanceLiveAssistantPlaybackSegmentIdle ReplayLiveAssistantPlaybackSegmentIdle unproven-1811
MeerkatMachine Idle input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByCloseIdle AuthorizeLiveContextAppendDeliveredReplayIdle unproven-1811
MeerkatMachine Idle input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByConversationIdle AuthorizeLiveContextAppendDeferredByTurnIdle unproven-1811
MeerkatMachine Idle input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByConversationIdle AuthorizeLiveContextAppendDeliveredReplayIdle unproven-1811
MeerkatMachine Idle input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByConversationIdle AuthorizeLiveContextAppendSupersededIdle unproven-1811
MeerkatMachine Idle input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByRecoveryIdle AuthorizeLiveContextAppendDeliveredReplayIdle unproven-1811
MeerkatMachine Idle input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByTurnIdle AuthorizeLiveContextAppendDeliveredReplayIdle unproven-1811
MeerkatMachine Idle input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendIdle AuthorizeLiveContextAppendSupersededIdle unproven-1811
MeerkatMachine Idle input:AuthorizeOperatorDurableTailRepair AuthorizeOperatorDurableTailRepairIdle AuthorizeOperatorDurableTailRepairRefuseIdle unproven-1811
MeerkatMachine Idle input:BeginImageOperation BeginImageOperationApprovalDeniedIdle BeginImageOperationScopedConflictIdle unproven-1811
MeerkatMachine Idle input:BeginImageOperation BeginImageOperationApprovalUnavailableIdle BeginImageOperationScopedConflictIdle unproven-1811
MeerkatMachine Idle input:CancelLiveRecoveryObligation CancelLiveContextRecoveryObligationIdle CancelLiveDelegationResultRecoveryObligationIdle unproven-1811
MeerkatMachine Idle input:ClaimModelRoutingHandoff ClaimModelRoutingHandoffAlreadyClaimedIdle ClaimModelRoutingHandoffAlreadyRealizedIdle unproven-1811
MeerkatMachine Idle input:ClaimModelRoutingHandoff ClaimModelRoutingHandoffAlreadyClaimedIdle ClaimModelRoutingHandoffImportedIdle unproven-1811
MeerkatMachine Idle input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalCancelledIdle ClassifyImageOperationTerminalRefusedIdle unproven-1811
MeerkatMachine Idle input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalCancelledIdle ClassifyImageOperationTerminalSafetyFilteredIdle unproven-1811
MeerkatMachine Idle input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalCancelledIdle ClassifyImageOperationTerminalTimeoutIdle unproven-1811
MeerkatMachine Idle input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalRefusedIdle ClassifyImageOperationTerminalTimeoutIdle unproven-1811
MeerkatMachine Idle input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalSafetyFilteredIdle ClassifyImageOperationTerminalTimeoutIdle unproven-1811
MeerkatMachine Idle input:ClassifyRecoveredOperationRecord ClassifyRecoveredOperationRecordCapacitySlotDiscardIdle ClassifyRecoveredOperationRecordDiscardIdle unproven-1811
MeerkatMachine Idle input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonIdle ClassifyStopInputDispositionRetainIdle unproven-1811
MeerkatMachine Idle input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonIdle ClassifyStopInputDispositionRetainResumableIdle unproven-1811
MeerkatMachine Idle input:ClassifyTerminalCompletionCorrelation ClassifyTerminalCompletionCorrelationAbandonedIdle ClassifyTerminalCompletionCorrelationRunIdle unproven-1811
MeerkatMachine Idle input:DenyModelRoutingHandoff DenyModelRoutingHandoffAlreadyDeniedIdle DenyModelRoutingHandoffPendingIdle unproven-1811
MeerkatMachine Idle input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedIdle ObserveSupervisorRotationNextPublishPendingIdle unproven-1811
MeerkatMachine Idle input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedIdle ObserveSupervisorRotationNotFoundIdle unproven-1811
MeerkatMachine Idle input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedIdle ObserveSupervisorRotationPreviousRevokePendingIdle unproven-1811
MeerkatMachine Idle input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingIdle ObserveSupervisorRotationNotFoundIdle unproven-1811
MeerkatMachine Idle input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingIdle ObserveSupervisorRotationRejectedIdle unproven-1811
MeerkatMachine Idle input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundIdle ObserveSupervisorRotationPreviousRevokePendingIdle unproven-1811
MeerkatMachine Idle input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundIdle ObserveSupervisorRotationRejectedIdle unproven-1811
MeerkatMachine Idle input:ObserveSupervisorRotation ObserveSupervisorRotationPreviousRevokePendingIdle ObserveSupervisorRotationRejectedIdle unproven-1811
MeerkatMachine Idle input:RealizeModelRoutingHandoff RealizeModelRoutingHandoffAlreadyRealizedIdle RealizeModelRoutingHandoffClaimedIdle unproven-1811
MeerkatMachine Idle input:ReleaseRunStarts ReleaseRunStartsLastIdle ReleaseRunStartsStillHeldIdle unproven-1811
MeerkatMachine Idle input:RequestFiniteSwitchTurn RequestFiniteSwitchTurnApprovalDeniedIdle RequestFiniteSwitchTurnScopedConflictIdle unproven-1811
MeerkatMachine Idle input:RequestFiniteSwitchTurn RequestFiniteSwitchTurnApprovalUnavailableIdle RequestFiniteSwitchTurnScopedConflictIdle unproven-1811
MeerkatMachine Idle input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionAcceptedIdle ResolveLiveWebrtcAnswerAdmissionChannelNotBoundIdle unproven-1811
MeerkatMachine Idle input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionChannelNotBoundIdle ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedIdle unproven-1811
MeerkatMachine Idle input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionChannelNotBoundIdle ResolveLiveWebrtcAnswerAdmissionTokenNotFoundIdle unproven-1811
MeerkatMachine Idle input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedIdle ResolveLiveWebrtcAnswerAdmissionTokenNotFoundIdle unproven-1811
MeerkatMachine Idle input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionAcceptedIdle ResolveLiveWebsocketTokenAdmissionChannelNotBoundIdle unproven-1811
MeerkatMachine Idle input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionChannelNotBoundIdle ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedIdle unproven-1811
MeerkatMachine Idle input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionChannelNotBoundIdle ResolveLiveWebsocketTokenAdmissionTokenNotFoundIdle unproven-1811
MeerkatMachine Idle input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedIdle ResolveLiveWebsocketTokenAdmissionTokenNotFoundIdle unproven-1811
MeerkatMachine Idle input:ResolveSupervisorAuthorizeAdmission ResolveSupervisorAuthorizeAdmissionIdempotentAckIdle ResolveSupervisorAuthorizeAdmissionProceedIdle unproven-1811
MeerkatMachine Idle input:ResolveSupervisorAuthorizeAdmission ResolveSupervisorAuthorizeAdmissionIdempotentAckIdle ResolveSupervisorAuthorizeAdmissionStaleSupervisorIdle unproven-1811
MeerkatMachine Idle input:ResolveVisibleRuntimePhase ResolveVisibleRuntimePhaseKeepDslNoRewriteIdle ResolveVisibleRuntimePhaseKeepDslVisibleRewriteIdle unproven-1811
MeerkatMachine Idle input:ResolveVisibleRuntimePhase ResolveVisibleRuntimePhaseKeepDslNoRewriteIdle ResolveVisibleRuntimePhasePublishControlNoRewriteIdle unproven-1811
MeerkatMachine Idle input:ResolveVisibleRuntimePhase ResolveVisibleRuntimePhaseKeepDslNoRewriteIdle ResolveVisibleRuntimePhasePublishControlVisibleRewriteIdle unproven-1811
MeerkatMachine Idle input:ResolveVisibleRuntimePhase ResolveVisibleRuntimePhaseKeepDslVisibleRewriteIdle ResolveVisibleRuntimePhasePublishControlNoRewriteIdle unproven-1811
MeerkatMachine Idle input:ResolveVisibleRuntimePhase ResolveVisibleRuntimePhaseKeepDslVisibleRewriteIdle ResolveVisibleRuntimePhasePublishControlVisibleRewriteIdle unproven-1811
MeerkatMachine Idle input:ResolveVisibleRuntimePhase ResolveVisibleRuntimePhasePublishControlNoRewriteIdle ResolveVisibleRuntimePhasePublishControlVisibleRewriteIdle unproven-1811
MeerkatMachine Idle input:RevokeLiveChannelCloseCustody RevokeLiveChannelCloseCustodyClosedReplayIdle RevokeLiveChannelCloseCustodyIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationAdoptCurrentIdle SubmitSupervisorRotationConflictIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationAdoptCurrentIdle SubmitSupervisorRotationNewIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationAdoptCurrentIdle SubmitSupervisorRotationPersistRejectedIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationConflictIdle SubmitSupervisorRotationExistingCompletedIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationConflictIdle SubmitSupervisorRotationExistingPendingIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationConflictIdle SubmitSupervisorRotationExistingRejectedIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationConflictIdle SubmitSupervisorRotationNewIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationConflictIdle SubmitSupervisorRotationPersistPreflightRejectedIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationConflictIdle SubmitSupervisorRotationPersistRejectedIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedIdle SubmitSupervisorRotationExistingPendingIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedIdle SubmitSupervisorRotationUnavailableIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationExistingPendingIdle SubmitSupervisorRotationExistingRejectedIdle unproven-1811
MeerkatMachine Idle input:SubmitSupervisorRotation SubmitSupervisorRotationExistingRejectedIdle SubmitSupervisorRotationUnavailableIdle unproven-1811
MeerkatMachine Idle input:SupersedeLiveInteraction SupersedeCompletedLiveInteractionDelegationWithCancellationIdle SupersedeLiveInteractionWithDelegationCancellationIdle unproven-1811
MeerkatMachine Idle input:SupersedeLiveInteraction SupersedeCompletedLiveInteractionDelegationWithCancellationIdle SupersedeLiveInteractionWithoutDelegationCancellationIdle unproven-1811
MeerkatMachine Idle input:SupersedeLiveInteraction SupersedeLiveInteractionWithDelegationCancellationIdle SupersedeLiveInteractionWithoutDelegationCancellationIdle unproven-1811
MeerkatMachine Initializing input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonInitializing ClassifyStopInputDispositionRetainInitializing unproven-1811
MeerkatMachine Initializing input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonInitializing ClassifyStopInputDispositionRetainResumableInitializing unproven-1811
MeerkatMachine Initializing input:ClassifyTerminalCompletionCorrelation ClassifyTerminalCompletionCorrelationAbandonedInitializing ClassifyTerminalCompletionCorrelationRunInitializing unproven-1811
MeerkatMachine Initializing input:ReleaseRunStarts ReleaseRunStartsLastInitializing ReleaseRunStartsStillHeldInitializing unproven-1811
MeerkatMachine Retired input:ArchiveUnresolvedModelRoutingHandoff ArchiveUnresolvedModelRoutingHandoffAlreadyArchivedRetired ArchiveUnresolvedModelRoutingHandoffPendingRetired unproven-1811
MeerkatMachine Retired input:AuthorizeOperatorDurableTailRepair AuthorizeOperatorDurableTailRepairRefuseRetired AuthorizeOperatorDurableTailRepairRetired unproven-1811
MeerkatMachine Retired input:CancelLiveRecoveryObligation CancelLiveContextRecoveryObligationRetired CancelLiveDelegationResultRecoveryObligationRetired unproven-1811
MeerkatMachine Retired input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonRetired ClassifyStopInputDispositionRetainResumableRetired unproven-1811
MeerkatMachine Retired input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonRetired ClassifyStopInputDispositionRetainRetired unproven-1811
MeerkatMachine Retired input:ClassifyTerminalCompletionCorrelation ClassifyTerminalCompletionCorrelationAbandonedRetired ClassifyTerminalCompletionCorrelationRunRetired unproven-1811
MeerkatMachine Retired input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedRetired ObserveSupervisorRotationNextPublishPendingRetired unproven-1811
MeerkatMachine Retired input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedRetired ObserveSupervisorRotationNotFoundRetired unproven-1811
MeerkatMachine Retired input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedRetired ObserveSupervisorRotationPreviousRevokePendingRetired unproven-1811
MeerkatMachine Retired input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingRetired ObserveSupervisorRotationNotFoundRetired unproven-1811
MeerkatMachine Retired input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingRetired ObserveSupervisorRotationRejectedRetired unproven-1811
MeerkatMachine Retired input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundRetired ObserveSupervisorRotationPreviousRevokePendingRetired unproven-1811
MeerkatMachine Retired input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundRetired ObserveSupervisorRotationRejectedRetired unproven-1811
MeerkatMachine Retired input:ObserveSupervisorRotation ObserveSupervisorRotationPreviousRevokePendingRetired ObserveSupervisorRotationRejectedRetired unproven-1811
MeerkatMachine Retired input:ReleaseRunStarts ReleaseRunStartsLastRetired ReleaseRunStartsStillHeldRetired unproven-1811
MeerkatMachine Retired input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionAcceptedRetired ResolveLiveWebrtcAnswerAdmissionChannelNotBoundRetired unproven-1811
MeerkatMachine Retired input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionChannelNotBoundRetired ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedRetired unproven-1811
MeerkatMachine Retired input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionChannelNotBoundRetired ResolveLiveWebrtcAnswerAdmissionTokenNotFoundRetired unproven-1811
MeerkatMachine Retired input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedRetired ResolveLiveWebrtcAnswerAdmissionTokenNotFoundRetired unproven-1811
MeerkatMachine Retired input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionAcceptedRetired ResolveLiveWebsocketTokenAdmissionChannelNotBoundRetired unproven-1811
MeerkatMachine Retired input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionChannelNotBoundRetired ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedRetired unproven-1811
MeerkatMachine Retired input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionChannelNotBoundRetired ResolveLiveWebsocketTokenAdmissionTokenNotFoundRetired unproven-1811
MeerkatMachine Retired input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedRetired ResolveLiveWebsocketTokenAdmissionTokenNotFoundRetired unproven-1811
MeerkatMachine Retired input:ResolveSupervisorAuthorizeAdmission ResolveSupervisorAuthorizeAdmissionIdempotentAckRetired ResolveSupervisorAuthorizeAdmissionStaleSupervisorRetired unproven-1811
MeerkatMachine Retired input:SubmitSupervisorRotation SubmitSupervisorRotationConflictRetired SubmitSupervisorRotationExistingCompletedRetired unproven-1811
MeerkatMachine Retired input:SubmitSupervisorRotation SubmitSupervisorRotationConflictRetired SubmitSupervisorRotationExistingPendingRetired unproven-1811
MeerkatMachine Retired input:SubmitSupervisorRotation SubmitSupervisorRotationConflictRetired SubmitSupervisorRotationExistingRejectedRetired unproven-1811
MeerkatMachine Retired input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedRetired SubmitSupervisorRotationExistingPendingRetired unproven-1811
MeerkatMachine Retired input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedRetired SubmitSupervisorRotationUnavailableRetired unproven-1811
MeerkatMachine Retired input:SubmitSupervisorRotation SubmitSupervisorRotationExistingPendingRetired SubmitSupervisorRotationExistingRejectedRetired unproven-1811
MeerkatMachine Retired input:SubmitSupervisorRotation SubmitSupervisorRotationExistingRejectedRetired SubmitSupervisorRotationUnavailableRetired unproven-1811
MeerkatMachine Running input:AdmitModelRoutingAssistantTurn AdmitPendingFiniteSwitchTurnRunning DecrementFiniteSwitchTurnRunning unproven-1811
MeerkatMachine Running input:AdmitModelRoutingAssistantTurn AdmitPendingFiniteSwitchTurnRunning RestoreConsumedFiniteSwitchTurnRunning unproven-1811
MeerkatMachine Running input:AdvanceLiveAssistantPlaybackSegment AdvanceLiveAssistantPlaybackSegmentRunning ReplayLiveAssistantPlaybackSegmentRunning unproven-1811
MeerkatMachine Running input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByCloseRunning AuthorizeLiveContextAppendDeliveredReplayRunning unproven-1811
MeerkatMachine Running input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByConversationRunning AuthorizeLiveContextAppendDeferredByTurnRunning unproven-1811
MeerkatMachine Running input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByConversationRunning AuthorizeLiveContextAppendDeliveredReplayRunning unproven-1811
MeerkatMachine Running input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByConversationRunning AuthorizeLiveContextAppendSupersededRunning unproven-1811
MeerkatMachine Running input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByRecoveryRunning AuthorizeLiveContextAppendDeliveredReplayRunning unproven-1811
MeerkatMachine Running input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendDeferredByTurnRunning AuthorizeLiveContextAppendDeliveredReplayRunning unproven-1811
MeerkatMachine Running input:AuthorizeLiveContextAppend AuthorizeLiveContextAppendRunning AuthorizeLiveContextAppendSupersededRunning unproven-1811
MeerkatMachine Running input:BeginImageOperation BeginImageOperationApprovalDeniedRunning BeginImageOperationScopedConflictRunning unproven-1811
MeerkatMachine Running input:BeginImageOperation BeginImageOperationApprovalUnavailableRunning BeginImageOperationScopedConflictRunning unproven-1811
MeerkatMachine Running input:CancelLiveRecoveryObligation CancelLiveContextRecoveryObligationRunning CancelLiveDelegationResultRecoveryObligationRunning unproven-1811
MeerkatMachine Running input:ClaimModelRoutingHandoff ClaimModelRoutingHandoffAlreadyClaimedRunning ClaimModelRoutingHandoffAlreadyRealizedRunning unproven-1811
MeerkatMachine Running input:ClaimModelRoutingHandoff ClaimModelRoutingHandoffAlreadyClaimedRunning ClaimModelRoutingHandoffImportedRunning unproven-1811
MeerkatMachine Running input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalCancelledRunning ClassifyImageOperationTerminalRefusedRunning unproven-1811
MeerkatMachine Running input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalCancelledRunning ClassifyImageOperationTerminalSafetyFilteredRunning unproven-1811
MeerkatMachine Running input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalCancelledRunning ClassifyImageOperationTerminalTimeoutRunning unproven-1811
MeerkatMachine Running input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalRefusedRunning ClassifyImageOperationTerminalTimeoutRunning unproven-1811
MeerkatMachine Running input:ClassifyImageOperationTerminal ClassifyImageOperationTerminalSafetyFilteredRunning ClassifyImageOperationTerminalTimeoutRunning unproven-1811
MeerkatMachine Running input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonRunning ClassifyStopInputDispositionRetainResumableRunning unproven-1811
MeerkatMachine Running input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonRunning ClassifyStopInputDispositionRetainRunning unproven-1811
MeerkatMachine Running input:ClassifyTerminalCompletionCorrelation ClassifyTerminalCompletionCorrelationAbandonedRunning ClassifyTerminalCompletionCorrelationRunRunning unproven-1811
MeerkatMachine Running input:DenyModelRoutingHandoff DenyModelRoutingHandoffAlreadyDeniedRunning DenyModelRoutingHandoffPendingRunning unproven-1811
MeerkatMachine Running input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedRunning ObserveSupervisorRotationNextPublishPendingRunning unproven-1811
MeerkatMachine Running input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedRunning ObserveSupervisorRotationNotFoundRunning unproven-1811
MeerkatMachine Running input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedRunning ObserveSupervisorRotationPreviousRevokePendingRunning unproven-1811
MeerkatMachine Running input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingRunning ObserveSupervisorRotationNotFoundRunning unproven-1811
MeerkatMachine Running input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingRunning ObserveSupervisorRotationRejectedRunning unproven-1811
MeerkatMachine Running input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundRunning ObserveSupervisorRotationPreviousRevokePendingRunning unproven-1811
MeerkatMachine Running input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundRunning ObserveSupervisorRotationRejectedRunning unproven-1811
MeerkatMachine Running input:ObserveSupervisorRotation ObserveSupervisorRotationPreviousRevokePendingRunning ObserveSupervisorRotationRejectedRunning unproven-1811
MeerkatMachine Running input:RealizeModelRoutingHandoff RealizeModelRoutingHandoffAlreadyRealizedRunning RealizeModelRoutingHandoffClaimedRunning unproven-1811
MeerkatMachine Running input:ReleaseRunStarts ReleaseRunStartsLastRunning ReleaseRunStartsStillHeldRunning unproven-1811
MeerkatMachine Running input:RequestFiniteSwitchTurn RequestFiniteSwitchTurnApprovalDeniedRunning RequestFiniteSwitchTurnScopedConflictRunning unproven-1811
MeerkatMachine Running input:RequestFiniteSwitchTurn RequestFiniteSwitchTurnApprovalUnavailableRunning RequestFiniteSwitchTurnScopedConflictRunning unproven-1811
MeerkatMachine Running input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionAcceptedRunning ResolveLiveWebrtcAnswerAdmissionChannelNotBoundRunning unproven-1811
MeerkatMachine Running input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionChannelNotBoundRunning ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedRunning unproven-1811
MeerkatMachine Running input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionChannelNotBoundRunning ResolveLiveWebrtcAnswerAdmissionTokenNotFoundRunning unproven-1811
MeerkatMachine Running input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedRunning ResolveLiveWebrtcAnswerAdmissionTokenNotFoundRunning unproven-1811
MeerkatMachine Running input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionAcceptedRunning ResolveLiveWebsocketTokenAdmissionChannelNotBoundRunning unproven-1811
MeerkatMachine Running input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionChannelNotBoundRunning ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedRunning unproven-1811
MeerkatMachine Running input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionChannelNotBoundRunning ResolveLiveWebsocketTokenAdmissionTokenNotFoundRunning unproven-1811
MeerkatMachine Running input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedRunning ResolveLiveWebsocketTokenAdmissionTokenNotFoundRunning unproven-1811
MeerkatMachine Running input:ResolveSupervisorAuthorizeAdmission ResolveSupervisorAuthorizeAdmissionIdempotentAckRunning ResolveSupervisorAuthorizeAdmissionProceedRunning unproven-1811
MeerkatMachine Running input:ResolveSupervisorAuthorizeAdmission ResolveSupervisorAuthorizeAdmissionIdempotentAckRunning ResolveSupervisorAuthorizeAdmissionStaleSupervisorRunning unproven-1811
MeerkatMachine Running input:RevokeLiveChannelCloseCustody RevokeLiveChannelCloseCustodyClosedReplayRunning RevokeLiveChannelCloseCustodyRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationAdoptCurrentRunning SubmitSupervisorRotationConflictRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationAdoptCurrentRunning SubmitSupervisorRotationNewRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationAdoptCurrentRunning SubmitSupervisorRotationPersistRejectedRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationConflictRunning SubmitSupervisorRotationExistingCompletedRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationConflictRunning SubmitSupervisorRotationExistingPendingRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationConflictRunning SubmitSupervisorRotationExistingRejectedRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationConflictRunning SubmitSupervisorRotationNewRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationConflictRunning SubmitSupervisorRotationPersistPreflightRejectedRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationConflictRunning SubmitSupervisorRotationPersistRejectedRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedRunning SubmitSupervisorRotationExistingPendingRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedRunning SubmitSupervisorRotationUnavailableRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationExistingPendingRunning SubmitSupervisorRotationExistingRejectedRunning unproven-1811
MeerkatMachine Running input:SubmitSupervisorRotation SubmitSupervisorRotationExistingRejectedRunning SubmitSupervisorRotationUnavailableRunning unproven-1811
MeerkatMachine Running input:SupersedeLiveInteraction SupersedeCompletedLiveInteractionDelegationWithCancellationRunning SupersedeLiveInteractionWithDelegationCancellationRunning unproven-1811
MeerkatMachine Running input:SupersedeLiveInteraction SupersedeCompletedLiveInteractionDelegationWithCancellationRunning SupersedeLiveInteractionWithoutDelegationCancellationRunning unproven-1811
MeerkatMachine Running input:SupersedeLiveInteraction SupersedeLiveInteractionWithDelegationCancellationRunning SupersedeLiveInteractionWithoutDelegationCancellationRunning unproven-1811
MeerkatMachine Stopped input:CancelLiveRecoveryObligation CancelLiveContextRecoveryObligationStopped CancelLiveDelegationResultRecoveryObligationStopped unproven-1811
MeerkatMachine Stopped input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonStopped ClassifyStopInputDispositionRetainResumableStopped unproven-1811
MeerkatMachine Stopped input:ClassifyStopInputDisposition ClassifyStopInputDispositionAbandonStopped ClassifyStopInputDispositionRetainStopped unproven-1811
MeerkatMachine Stopped input:ClassifyTerminalCompletionCorrelation ClassifyTerminalCompletionCorrelationAbandonedStopped ClassifyTerminalCompletionCorrelationRunStopped unproven-1811
MeerkatMachine Stopped input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedStopped ObserveSupervisorRotationNextPublishPendingStopped unproven-1811
MeerkatMachine Stopped input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedStopped ObserveSupervisorRotationNotFoundStopped unproven-1811
MeerkatMachine Stopped input:ObserveSupervisorRotation ObserveSupervisorRotationCompletedStopped ObserveSupervisorRotationPreviousRevokePendingStopped unproven-1811
MeerkatMachine Stopped input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingStopped ObserveSupervisorRotationNotFoundStopped unproven-1811
MeerkatMachine Stopped input:ObserveSupervisorRotation ObserveSupervisorRotationNextPublishPendingStopped ObserveSupervisorRotationRejectedStopped unproven-1811
MeerkatMachine Stopped input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundStopped ObserveSupervisorRotationPreviousRevokePendingStopped unproven-1811
MeerkatMachine Stopped input:ObserveSupervisorRotation ObserveSupervisorRotationNotFoundStopped ObserveSupervisorRotationRejectedStopped unproven-1811
MeerkatMachine Stopped input:ObserveSupervisorRotation ObserveSupervisorRotationPreviousRevokePendingStopped ObserveSupervisorRotationRejectedStopped unproven-1811
MeerkatMachine Stopped input:ReleaseRunStarts ReleaseRunStartsLastStopped ReleaseRunStartsStillHeldStopped unproven-1811
MeerkatMachine Stopped input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionAcceptedStopped ResolveLiveWebrtcAnswerAdmissionChannelNotBoundStopped unproven-1811
MeerkatMachine Stopped input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionChannelNotBoundStopped ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedStopped unproven-1811
MeerkatMachine Stopped input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionChannelNotBoundStopped ResolveLiveWebrtcAnswerAdmissionTokenNotFoundStopped unproven-1811
MeerkatMachine Stopped input:ResolveLiveWebrtcAnswerAdmission ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedStopped ResolveLiveWebrtcAnswerAdmissionTokenNotFoundStopped unproven-1811
MeerkatMachine Stopped input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionAcceptedStopped ResolveLiveWebsocketTokenAdmissionChannelNotBoundStopped unproven-1811
MeerkatMachine Stopped input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionChannelNotBoundStopped ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedStopped unproven-1811
MeerkatMachine Stopped input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionChannelNotBoundStopped ResolveLiveWebsocketTokenAdmissionTokenNotFoundStopped unproven-1811
MeerkatMachine Stopped input:ResolveLiveWebsocketTokenAdmission ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedStopped ResolveLiveWebsocketTokenAdmissionTokenNotFoundStopped unproven-1811
MeerkatMachine Stopped input:ResolveSupervisorAuthorizeAdmission ResolveSupervisorAuthorizeAdmissionIdempotentAckStopped ResolveSupervisorAuthorizeAdmissionStaleSupervisorStopped unproven-1811
MeerkatMachine Stopped input:SubmitSupervisorRotation SubmitSupervisorRotationConflictStopped SubmitSupervisorRotationExistingCompletedStopped unproven-1811
MeerkatMachine Stopped input:SubmitSupervisorRotation SubmitSupervisorRotationConflictStopped SubmitSupervisorRotationExistingPendingStopped unproven-1811
MeerkatMachine Stopped input:SubmitSupervisorRotation SubmitSupervisorRotationConflictStopped SubmitSupervisorRotationExistingRejectedStopped unproven-1811
MeerkatMachine Stopped input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedStopped SubmitSupervisorRotationExistingPendingStopped unproven-1811
MeerkatMachine Stopped input:SubmitSupervisorRotation SubmitSupervisorRotationExistingCompletedStopped SubmitSupervisorRotationUnavailableStopped unproven-1811
MeerkatMachine Stopped input:SubmitSupervisorRotation SubmitSupervisorRotationExistingPendingStopped SubmitSupervisorRotationExistingRejectedStopped unproven-1811
MeerkatMachine Stopped input:SubmitSupervisorRotation SubmitSupervisorRotationExistingRejectedStopped SubmitSupervisorRotationUnavailableStopped unproven-1811
MobMachine Completed input:AuthorizeMobEventRouterMemberRemoval AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveCompleted AuthorizeMobEventRouterMemberRemovalUnboundCompleted unproven-1811
MobMachine Completed input:CommitRemoteTurnOutcome CommitRemoteTurnOutcomePendingCompleted CommitRemoteTurnOutcomeReplayCompleted unproven-1811
MobMachine Completed input:RecordPlacedCompletionObligation RecordPlacedCompletionObligationPendingReplayCompleted RecordPlacedCompletionObligationResolvedReplayCompleted unproven-1811
MobMachine Completed input:RejectPlacedKickoffBeforeAdmission RejectPlacedKickoffBeforeAdmissionAfterCancellationCompleted RejectPlacedKickoffBeforeAdmissionFreshCompleted unproven-1811
MobMachine Completed input:RequestPlacedCompletionCancellation RequestPlacedCompletionCancellationReplayCompleted RequestPlacedCompletionCancellationTerminalReplayCompleted unproven-1811
MobMachine Completed input:ResolveAutonomousShutdownMemberAction ResolveAutonomousShutdownMemberActionLocalTerminalRetryAnchorCompleted ResolveAutonomousShutdownMemberActionPlacedTerminalRetryAnchorCompleted unproven-1811
MobMachine Completed input:ResolvePlacedCompletionOutcome ResolvePlacedCompletionOutcomePendingCompleted ResolvePlacedCompletionOutcomeReplayCompleted unproven-1811
MobMachine Completed input:ResolvePlacedKickoffCallbackPending ResolvePlacedKickoffCallbackPendingAfterCancellationCompleted ResolvePlacedKickoffCallbackPendingFreshCompleted unproven-1811
MobMachine Completed input:ResolvePlacedKickoffCallbackPending ResolvePlacedKickoffCallbackPendingAfterCancellationCompleted ResolvePlacedKickoffCallbackPendingReplayCompleted unproven-1811
MobMachine Completed input:ResolvePlacedKickoffCallbackPending ResolvePlacedKickoffCallbackPendingFreshCompleted ResolvePlacedKickoffCallbackPendingReplayCompleted unproven-1811
MobMachine Completed input:ResolvePlacedKickoffCancelled ResolvePlacedKickoffCancelledFreshCompleted ResolvePlacedKickoffCancelledReplayCompleted unproven-1811
MobMachine Completed input:ResolvePlacedKickoffFailed ResolvePlacedKickoffFailedAfterCancellationCompleted ResolvePlacedKickoffFailedFreshCompleted unproven-1811
MobMachine Completed input:ResolvePlacedKickoffFailed ResolvePlacedKickoffFailedAfterCancellationCompleted ResolvePlacedKickoffFailedReplayCompleted unproven-1811
MobMachine Completed input:ResolvePlacedKickoffFailed ResolvePlacedKickoffFailedFreshCompleted ResolvePlacedKickoffFailedReplayCompleted unproven-1811
MobMachine Completed input:ResolvePlacedKickoffStarted ResolvePlacedKickoffStartedAfterCancellationCompleted ResolvePlacedKickoffStartedFreshCompleted unproven-1811
MobMachine Completed input:ResolvePlacedKickoffStarted ResolvePlacedKickoffStartedAfterCancellationCompleted ResolvePlacedKickoffStartedReplayCompleted unproven-1811
MobMachine Completed input:ResolvePlacedKickoffStarted ResolvePlacedKickoffStartedFreshCompleted ResolvePlacedKickoffStartedReplayCompleted unproven-1811
MobMachine Completed input:ResolveRemoteTurnObligation ResolveRemoteTurnObligationCommittedCompleted ResolveRemoteTurnObligationReplayCompleted unproven-1811
MobMachine Completed input:StartPlacedKickoff StartPlacedKickoffPendingReplayCompleted StartPlacedKickoffResolvedReplayCompleted unproven-1811
MobMachine Destroyed input:AuthorizeMobEventRouterMemberRemoval AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveDestroyed AuthorizeMobEventRouterMemberRemovalUnboundDestroyed unproven-1811
MobMachine Destroyed input:CommitRemoteTurnOutcome CommitRemoteTurnOutcomePendingDestroyed CommitRemoteTurnOutcomeReplayDestroyed unproven-1811
MobMachine Destroyed input:ResolveRemoteTurnObligation ResolveRemoteTurnObligationCommittedDestroyed ResolveRemoteTurnObligationReplayDestroyed unproven-1811
MobMachine Running input:AuthorizeMobEventRouterMemberRemoval AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveRunning AuthorizeMobEventRouterMemberRemovalUnboundRunning unproven-1811
MobMachine Running input:BeginHostBind BeginHostBindFreshReplacement BeginHostBindFreshUnplaced unproven-1811
MobMachine Running input:CommitRemoteTurnOutcome CommitRemoteTurnOutcomePendingRunning CommitRemoteTurnOutcomeReplayRunning unproven-1811
MobMachine Running input:KickoffMarkStarting KickoffMarkStarting KickoffMarkStartingReplayRunning unproven-1811
MobMachine Running input:ObserveMemberProgress ObserveMemberProgressUnchangedOpenHealthyRunning ObserveMemberProgressUnchangedOpenWedgedRunning unproven-1811
MobMachine Running input:RecordPlacedCompletionObligation RecordPlacedCompletionObligationFresh RecordPlacedCompletionObligationPendingReplayRunning unproven-1811
MobMachine Running input:RecordPlacedCompletionObligation RecordPlacedCompletionObligationFresh RecordPlacedCompletionObligationResolvedReplayRunning unproven-1811
MobMachine Running input:RecordPlacedCompletionObligation RecordPlacedCompletionObligationPendingReplayRunning RecordPlacedCompletionObligationResolvedReplayRunning unproven-1811
MobMachine Running input:RecordRemoteTurnObligation RecordRemoteTurnObligationCommittedReplay RecordRemoteTurnObligationPendingReplay unproven-1811
MobMachine Running input:RecordRemoteTurnObligation RecordRemoteTurnObligationCommittedReplay RecordRemoteTurnObligationResolvedReplay unproven-1811
MobMachine Running input:RecordRemoteTurnObligation RecordRemoteTurnObligationPendingReplay RecordRemoteTurnObligationResolvedReplay unproven-1811
MobMachine Running input:RejectPlacedKickoffBeforeAdmission RejectPlacedKickoffBeforeAdmissionAfterCancellationRunning RejectPlacedKickoffBeforeAdmissionFreshRunning unproven-1811
MobMachine Running input:RequestPlacedCompletionCancellation RequestPlacedCompletionCancellationReplayRunning RequestPlacedCompletionCancellationTerminalReplayRunning unproven-1811
MobMachine Running input:ResolveAutonomousShutdownMemberAction ResolveAutonomousShutdownMemberActionLocalTerminalRetryAnchorRunning ResolveAutonomousShutdownMemberActionPlacedTerminalRetryAnchorRunning unproven-1811
MobMachine Running input:ResolvePlacedCompletionOutcome ResolvePlacedCompletionOutcomePendingRunning ResolvePlacedCompletionOutcomeReplayRunning unproven-1811
MobMachine Running input:ResolvePlacedKickoffCallbackPending ResolvePlacedKickoffCallbackPendingAfterCancellationRunning ResolvePlacedKickoffCallbackPendingFreshRunning unproven-1811
MobMachine Running input:ResolvePlacedKickoffCallbackPending ResolvePlacedKickoffCallbackPendingAfterCancellationRunning ResolvePlacedKickoffCallbackPendingReplayRunning unproven-1811
MobMachine Running input:ResolvePlacedKickoffCallbackPending ResolvePlacedKickoffCallbackPendingFreshRunning ResolvePlacedKickoffCallbackPendingReplayRunning unproven-1811
MobMachine Running input:ResolvePlacedKickoffCancelled ResolvePlacedKickoffCancelledFreshRunning ResolvePlacedKickoffCancelledReplayRunning unproven-1811
MobMachine Running input:ResolvePlacedKickoffFailed ResolvePlacedKickoffFailedAfterCancellationRunning ResolvePlacedKickoffFailedFreshRunning unproven-1811
MobMachine Running input:ResolvePlacedKickoffFailed ResolvePlacedKickoffFailedAfterCancellationRunning ResolvePlacedKickoffFailedReplayRunning unproven-1811
MobMachine Running input:ResolvePlacedKickoffFailed ResolvePlacedKickoffFailedFreshRunning ResolvePlacedKickoffFailedReplayRunning unproven-1811
MobMachine Running input:ResolvePlacedKickoffStarted ResolvePlacedKickoffStartedAfterCancellationRunning ResolvePlacedKickoffStartedFreshRunning unproven-1811
MobMachine Running input:ResolvePlacedKickoffStarted ResolvePlacedKickoffStartedAfterCancellationRunning ResolvePlacedKickoffStartedReplayRunning unproven-1811
MobMachine Running input:ResolvePlacedKickoffStarted ResolvePlacedKickoffStartedFreshRunning ResolvePlacedKickoffStartedReplayRunning unproven-1811
MobMachine Running input:ResolveRemoteTurnObligation ResolveRemoteTurnObligationCommittedRunning ResolveRemoteTurnObligationReplayRunning unproven-1811
MobMachine Running input:ResolveSubmitWorkRejection ResolveSubmitWorkRejectionCurrentRuntimeNotLive ResolveSubmitWorkRejectionNotExternallyAddressable unproven-1811
MobMachine Running input:ResolveSubmitWorkRejection ResolveSubmitWorkRejectionCurrentRuntimeNotLive ResolveSubmitWorkRejectionPeerOnlyNotExternallyAddressable unproven-1811
MobMachine Running input:ResolveSubmitWorkRejection ResolveSubmitWorkRejectionCurrentRuntimeNotLive ResolveSubmitWorkRejectionRetiringAsMemberNotFound unproven-1811
MobMachine Running input:ResolveSubmitWorkRejection ResolveSubmitWorkRejectionCurrentRuntimeNotLive ResolveSubmitWorkRejectionStaleSessionBinding unproven-1811
MobMachine Running input:ResolveSubmitWorkRejection ResolveSubmitWorkRejectionNotExternallyAddressable ResolveSubmitWorkRejectionStaleFenceToken unproven-1811
MobMachine Running input:ResolveSubmitWorkRejection ResolveSubmitWorkRejectionNotExternallyAddressable ResolveSubmitWorkRejectionStaleSessionBinding unproven-1811
MobMachine Running input:ResolveSubmitWorkRejection ResolveSubmitWorkRejectionPeerOnlyNotExternallyAddressable ResolveSubmitWorkRejectionStaleFenceToken unproven-1811
MobMachine Running input:ResolveSubmitWorkRejection ResolveSubmitWorkRejectionPeerOnlyNotExternallyAddressable ResolveSubmitWorkRejectionStaleSessionBinding unproven-1811
MobMachine Running input:ResolveSubmitWorkRejection ResolveSubmitWorkRejectionRetiringAsMemberNotFound ResolveSubmitWorkRejectionStaleFenceToken unproven-1811
MobMachine Running input:ResolveSubmitWorkRejection ResolveSubmitWorkRejectionRetiringAsMemberNotFound ResolveSubmitWorkRejectionStaleSessionBinding unproven-1811
MobMachine Running input:ResolveSubmitWorkRejection ResolveSubmitWorkRejectionStaleFenceToken ResolveSubmitWorkRejectionStaleSessionBinding unproven-1811
MobMachine Running input:Retire RetireRemoteConfirmedRevokedRunning RetireRemoteReleasingRunning unproven-1811
MobMachine Running input:StartPlacedKickoff StartPlacedKickoffFinalReplayRunning StartPlacedKickoffFresh unproven-1811
MobMachine Running input:StartPlacedKickoff StartPlacedKickoffPendingReplayRunning StartPlacedKickoffResolvedReplayRunning unproven-1811
MobMachine Running signal:AdmitDestroyMemberRetire AdmitDestroyMemberRetireAlreadyRetiringReleasedSessionRunning AdmitDestroyMemberRetireAlreadyRetiringSessionRunning unproven-1811
MobMachine Running signal:AdmitDestroyMemberRetire AdmitDestroyMemberRetireConfirmedRevokedRunning AdmitDestroyMemberRetireRemoteRunning unproven-1811
MobMachine Running signal:RecoverMemberSessionBinding RecoverMemberSessionBindingAlreadyCurrentRunning RecoverMemberSessionBindingReplacingRunning unproven-1811
MobMachine Running signal:RecoverRosterMemberRetirementStarted RecoverRosterMemberRetirementStartedReleasing RecoverRosterMemberRetirementStartedReleasingAlreadyApplied unproven-1811
MobMachine Running signal:RecoverRosterMemberRetiring RecoverRosterMemberRetiringReplay RecoverRosterMemberRetiringRunning unproven-1811
MobMachine Running signal:RetireMember RetireMemberRemote RetireMemberRemoteConfirmedRevoked unproven-1811
MobMachine Stopped input:AuthorizeMobEventRouterMemberRemoval AuthorizeMobEventRouterMemberRemovalRuntimeNotLiveStopped AuthorizeMobEventRouterMemberRemovalUnboundStopped unproven-1811
MobMachine Stopped input:CommitRemoteTurnOutcome CommitRemoteTurnOutcomePendingStopped CommitRemoteTurnOutcomeReplayStopped unproven-1811
MobMachine Stopped input:RecordPlacedCompletionObligation RecordPlacedCompletionObligationPendingReplayStopped RecordPlacedCompletionObligationResolvedReplayStopped unproven-1811
MobMachine Stopped input:RejectPlacedKickoffBeforeAdmission RejectPlacedKickoffBeforeAdmissionAfterCancellationStopped RejectPlacedKickoffBeforeAdmissionFreshStopped unproven-1811
MobMachine Stopped input:RequestPlacedCompletionCancellation RequestPlacedCompletionCancellationReplayStopped RequestPlacedCompletionCancellationTerminalReplayStopped unproven-1811
MobMachine Stopped input:ResolveAutonomousShutdownMemberAction ResolveAutonomousShutdownMemberActionLocalTerminalRetryAnchorStopped ResolveAutonomousShutdownMemberActionPlacedTerminalRetryAnchorStopped unproven-1811
MobMachine Stopped input:ResolvePlacedCompletionOutcome ResolvePlacedCompletionOutcomePendingStopped ResolvePlacedCompletionOutcomeReplayStopped unproven-1811
MobMachine Stopped input:ResolvePlacedKickoffCallbackPending ResolvePlacedKickoffCallbackPendingAfterCancellationStopped ResolvePlacedKickoffCallbackPendingFreshStopped unproven-1811
MobMachine Stopped input:ResolvePlacedKickoffCallbackPending ResolvePlacedKickoffCallbackPendingAfterCancellationStopped ResolvePlacedKickoffCallbackPendingReplayStopped unproven-1811
MobMachine Stopped input:ResolvePlacedKickoffCallbackPending ResolvePlacedKickoffCallbackPendingFreshStopped ResolvePlacedKickoffCallbackPendingReplayStopped unproven-1811
MobMachine Stopped input:ResolvePlacedKickoffCancelled ResolvePlacedKickoffCancelledFreshStopped ResolvePlacedKickoffCancelledReplayStopped unproven-1811
MobMachine Stopped input:ResolvePlacedKickoffFailed ResolvePlacedKickoffFailedAfterCancellationStopped ResolvePlacedKickoffFailedFreshStopped unproven-1811
MobMachine Stopped input:ResolvePlacedKickoffFailed ResolvePlacedKickoffFailedAfterCancellationStopped ResolvePlacedKickoffFailedReplayStopped unproven-1811
MobMachine Stopped input:ResolvePlacedKickoffFailed ResolvePlacedKickoffFailedFreshStopped ResolvePlacedKickoffFailedReplayStopped unproven-1811
MobMachine Stopped input:ResolvePlacedKickoffStarted ResolvePlacedKickoffStartedAfterCancellationStopped ResolvePlacedKickoffStartedFreshStopped unproven-1811
MobMachine Stopped input:ResolvePlacedKickoffStarted ResolvePlacedKickoffStartedAfterCancellationStopped ResolvePlacedKickoffStartedReplayStopped unproven-1811
MobMachine Stopped input:ResolvePlacedKickoffStarted ResolvePlacedKickoffStartedFreshStopped ResolvePlacedKickoffStartedReplayStopped unproven-1811
MobMachine Stopped input:ResolveRemoteTurnObligation ResolveRemoteTurnObligationCommittedStopped ResolveRemoteTurnObligationReplayStopped unproven-1811
MobMachine Stopped input:Retire RetireRemoteConfirmedRevokedStopped RetireRemoteReleasingStopped unproven-1811
MobMachine Stopped input:StartPlacedKickoff StartPlacedKickoffPendingReplayStopped StartPlacedKickoffResolvedReplayStopped unproven-1811
MobMachine Stopped signal:AdmitDestroyMemberRetire AdmitDestroyMemberRetireAlreadyRetiringReleasedSessionStopped AdmitDestroyMemberRetireAlreadyRetiringSessionStopped unproven-1811
MobMachine Stopped signal:AdmitDestroyMemberRetire AdmitDestroyMemberRetireConfirmedRevokedStopped AdmitDestroyMemberRetireRemoteStopped unproven-1811
RuntimeDeliveryMachine Active input:ClassifyDeliveryStatus ClassifyAcknowledgedAhead ClassifyApplied unproven-1811
RuntimeDeliveryMachine Active input:ClassifyDeliveryStatus ClassifyAcknowledgedAhead ClassifyMixed unproven-1811
RuntimeDeliveryMachine Active input:ClassifyDeliveryStatus ClassifyAcknowledgedAhead ClassifyRefused unproven-1811
RuntimeDeliveryMachine Active input:ClassifyDeliveryStatus ClassifyApplied ClassifyPending unproven-1811
RuntimeDeliveryMachine Active input:ClassifyDeliveryStatus ClassifyMixed ClassifyPending unproven-1811
RuntimeDeliveryMachine Active input:ClassifyDeliveryStatus ClassifyPending ClassifyRefused unproven-1811
RuntimeDeliveryMachine Active input:FinishDeliveryRecipients FinishSettledDeliveryRecipients ObserveFinishedDeliveryRecipients unproven-1811
RuntimeDeliveryMachine Active input:SettleRefusedDelivery ObserveAlreadyRefusedDelivery SettleRefusedDeliveryAtCursor invariant:refused_deliveries_are_committed_and_passed
SessionDocumentMachine Ready input:AuthorizeSessionResumeOverrides AuthorizeSessionResumeOverridesAcceptRecomputeProvider AuthorizeSessionResumeOverridesAcceptRetainStored unproven-1811
SessionDocumentMachine Ready input:AuthorizeSessionResumeOverrides AuthorizeSessionResumeOverridesAcceptRecomputeProvider AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn unproven-1811
SessionDocumentMachine Ready input:AuthorizeSessionResumeOverrides AuthorizeSessionResumeOverridesAcceptRecomputeProviderWithSelfHostedOverride AuthorizeSessionResumeOverridesAcceptRetainStoredWithSelfHostedOverride unproven-1811
SessionDocumentMachine Ready input:AuthorizeSessionResumeOverrides AuthorizeSessionResumeOverridesAcceptRecomputeProviderWithSelfHostedOverride AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn unproven-1811
SessionDocumentMachine Ready input:AuthorizeSessionResumeOverrides AuthorizeSessionResumeOverridesAcceptRetainStored AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn unproven-1811
SessionDocumentMachine Ready input:AuthorizeSessionResumeOverrides AuthorizeSessionResumeOverridesAcceptRetainStoredWithSelfHostedOverride AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn unproven-1811
SessionDocumentMachine Ready input:AuthorizeSessionResumeOverrides AuthorizeSessionResumeOverridesAcceptUseOverride AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn unproven-1811
SessionDocumentMachine Ready input:AuthorizeSessionResumeOverrides AuthorizeSessionResumeOverridesAcceptUseOverride AuthorizeSessionResumeOverridesRejectProviderRequiresModel unproven-1811
SessionDocumentMachine Ready input:AuthorizeSessionResumeOverrides AuthorizeSessionResumeOverridesAcceptUseOverrideWithSelfHostedOverride AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn unproven-1811
SessionDocumentMachine Ready input:AuthorizeSessionResumeOverrides AuthorizeSessionResumeOverridesAcceptUseOverrideWithSelfHostedOverride AuthorizeSessionResumeOverridesRejectProviderRequiresModel unproven-1811
SessionDocumentMachine Ready input:AuthorizeSessionResumeOverrides AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn AuthorizeSessionResumeOverridesRejectProviderRequiresModel unproven-1811
SessionDocumentMachine Ready input:ClassifyDurableTail ClassifyDurableTailAmbiguous ClassifyDurableTailCompleted unproven-1811
SessionDocumentMachine Ready input:ResolveRealtimeAssistantDelta ResolveRealtimeAssistantDeltaAccepted ResolveRealtimeAssistantDeltaInvalidOrDuplicate unproven-1811
SessionDocumentMachine Ready input:ResolveRealtimeAssistantDelta ResolveRealtimeAssistantDeltaDiscarded ResolveRealtimeAssistantDeltaInvalidOrDuplicate unproven-1811
SessionDocumentMachine Ready input:ResolveRealtimeAssistantDelta ResolveRealtimeAssistantDeltaInvalidOrDuplicate ResolveRealtimeAssistantDeltaLaneConflict unproven-1811
SessionDocumentMachine Ready input:StageSessionInitialPrompt StageSessionInitialPromptClear StageSessionInitialPromptStore unproven-1811
WorkGraphLifecycleMachine Absent input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedAbsent ClassifyConfirmationAdmissionPrincipalKindMismatchAbsent unproven-1811
WorkGraphLifecycleMachine Absent input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedAbsent ClassifyConfirmationAdmissionPrincipalRequiredAbsent unproven-1811
WorkGraphLifecycleMachine Absent input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedAbsent ClassifyConfirmationAdmissionSelfAttestEmptyAbsent unproven-1811
WorkGraphLifecycleMachine Absent input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedAbsent ClassifyConfirmationAdmissionSupervisorMismatchAbsent unproven-1811
WorkGraphLifecycleMachine Absent input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindAbsent ClassifyConfirmationAdmissionPrincipalKindMismatchAbsent unproven-1811
WorkGraphLifecycleMachine Absent input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindAbsent ClassifyConfirmationAdmissionPrincipalRequiredAbsent unproven-1811
WorkGraphLifecycleMachine Absent input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindAbsent ClassifyConfirmationAdmissionSelfAttestEmptyAbsent unproven-1811
WorkGraphLifecycleMachine Absent input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindAbsent ClassifyConfirmationAdmissionSupervisorMismatchAbsent unproven-1811
WorkGraphLifecycleMachine Blocked input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedBlocked ClassifyConfirmationAdmissionPrincipalKindMismatchBlocked unproven-1811
WorkGraphLifecycleMachine Blocked input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedBlocked ClassifyConfirmationAdmissionPrincipalRequiredBlocked unproven-1811
WorkGraphLifecycleMachine Blocked input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedBlocked ClassifyConfirmationAdmissionSelfAttestEmptyBlocked unproven-1811
WorkGraphLifecycleMachine Blocked input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedBlocked ClassifyConfirmationAdmissionSupervisorMismatchBlocked unproven-1811
WorkGraphLifecycleMachine Blocked input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindBlocked ClassifyConfirmationAdmissionPrincipalKindMismatchBlocked unproven-1811
WorkGraphLifecycleMachine Blocked input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindBlocked ClassifyConfirmationAdmissionPrincipalRequiredBlocked unproven-1811
WorkGraphLifecycleMachine Blocked input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindBlocked ClassifyConfirmationAdmissionSelfAttestEmptyBlocked unproven-1811
WorkGraphLifecycleMachine Blocked input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindBlocked ClassifyConfirmationAdmissionSupervisorMismatchBlocked unproven-1811
WorkGraphLifecycleMachine Blocked input:PolicyEscalate PolicyEscalateBlockedAdmitted PolicyEscalateBlockedDenied unproven-1811
WorkGraphLifecycleMachine Cancelled input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedCancelled ClassifyConfirmationAdmissionPrincipalKindMismatchCancelled unproven-1811
WorkGraphLifecycleMachine Cancelled input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedCancelled ClassifyConfirmationAdmissionPrincipalRequiredCancelled unproven-1811
WorkGraphLifecycleMachine Cancelled input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedCancelled ClassifyConfirmationAdmissionSelfAttestEmptyCancelled unproven-1811
WorkGraphLifecycleMachine Cancelled input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedCancelled ClassifyConfirmationAdmissionSupervisorMismatchCancelled unproven-1811
WorkGraphLifecycleMachine Cancelled input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindCancelled ClassifyConfirmationAdmissionPrincipalKindMismatchCancelled unproven-1811
WorkGraphLifecycleMachine Cancelled input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindCancelled ClassifyConfirmationAdmissionPrincipalRequiredCancelled unproven-1811
WorkGraphLifecycleMachine Cancelled input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindCancelled ClassifyConfirmationAdmissionSelfAttestEmptyCancelled unproven-1811
WorkGraphLifecycleMachine Cancelled input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindCancelled ClassifyConfirmationAdmissionSupervisorMismatchCancelled unproven-1811
WorkGraphLifecycleMachine Completed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedCompleted ClassifyConfirmationAdmissionPrincipalKindMismatchCompleted unproven-1811
WorkGraphLifecycleMachine Completed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedCompleted ClassifyConfirmationAdmissionPrincipalRequiredCompleted unproven-1811
WorkGraphLifecycleMachine Completed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedCompleted ClassifyConfirmationAdmissionSelfAttestEmptyCompleted unproven-1811
WorkGraphLifecycleMachine Completed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedCompleted ClassifyConfirmationAdmissionSupervisorMismatchCompleted unproven-1811
WorkGraphLifecycleMachine Completed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindCompleted ClassifyConfirmationAdmissionPrincipalKindMismatchCompleted unproven-1811
WorkGraphLifecycleMachine Completed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindCompleted ClassifyConfirmationAdmissionPrincipalRequiredCompleted unproven-1811
WorkGraphLifecycleMachine Completed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindCompleted ClassifyConfirmationAdmissionSelfAttestEmptyCompleted unproven-1811
WorkGraphLifecycleMachine Completed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindCompleted ClassifyConfirmationAdmissionSupervisorMismatchCompleted unproven-1811
WorkGraphLifecycleMachine Failed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedFailed ClassifyConfirmationAdmissionPrincipalKindMismatchFailed unproven-1811
WorkGraphLifecycleMachine Failed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedFailed ClassifyConfirmationAdmissionPrincipalRequiredFailed unproven-1811
WorkGraphLifecycleMachine Failed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedFailed ClassifyConfirmationAdmissionSelfAttestEmptyFailed unproven-1811
WorkGraphLifecycleMachine Failed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedFailed ClassifyConfirmationAdmissionSupervisorMismatchFailed unproven-1811
WorkGraphLifecycleMachine Failed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindFailed ClassifyConfirmationAdmissionPrincipalKindMismatchFailed unproven-1811
WorkGraphLifecycleMachine Failed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindFailed ClassifyConfirmationAdmissionPrincipalRequiredFailed unproven-1811
WorkGraphLifecycleMachine Failed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindFailed ClassifyConfirmationAdmissionSelfAttestEmptyFailed unproven-1811
WorkGraphLifecycleMachine Failed input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindFailed ClassifyConfirmationAdmissionSupervisorMismatchFailed unproven-1811
WorkGraphLifecycleMachine InProgress input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedInProgress ClassifyConfirmationAdmissionPrincipalKindMismatchInProgress unproven-1811
WorkGraphLifecycleMachine InProgress input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedInProgress ClassifyConfirmationAdmissionPrincipalRequiredInProgress unproven-1811
WorkGraphLifecycleMachine InProgress input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedInProgress ClassifyConfirmationAdmissionSelfAttestEmptyInProgress unproven-1811
WorkGraphLifecycleMachine InProgress input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedInProgress ClassifyConfirmationAdmissionSupervisorMismatchInProgress unproven-1811
WorkGraphLifecycleMachine InProgress input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindInProgress ClassifyConfirmationAdmissionPrincipalKindMismatchInProgress unproven-1811
WorkGraphLifecycleMachine InProgress input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindInProgress ClassifyConfirmationAdmissionPrincipalRequiredInProgress unproven-1811
WorkGraphLifecycleMachine InProgress input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindInProgress ClassifyConfirmationAdmissionSelfAttestEmptyInProgress unproven-1811
WorkGraphLifecycleMachine InProgress input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindInProgress ClassifyConfirmationAdmissionSupervisorMismatchInProgress unproven-1811
WorkGraphLifecycleMachine InProgress input:PolicyEscalate PolicyEscalateInProgressAdmitted PolicyEscalateInProgressDenied unproven-1811
WorkGraphLifecycleMachine Open input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedOpen ClassifyConfirmationAdmissionPrincipalKindMismatchOpen unproven-1811
WorkGraphLifecycleMachine Open input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedOpen ClassifyConfirmationAdmissionPrincipalRequiredOpen unproven-1811
WorkGraphLifecycleMachine Open input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedOpen ClassifyConfirmationAdmissionSelfAttestEmptyOpen unproven-1811
WorkGraphLifecycleMachine Open input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionAdmittedOpen ClassifyConfirmationAdmissionSupervisorMismatchOpen unproven-1811
WorkGraphLifecycleMachine Open input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindOpen ClassifyConfirmationAdmissionPrincipalKindMismatchOpen unproven-1811
WorkGraphLifecycleMachine Open input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindOpen ClassifyConfirmationAdmissionPrincipalRequiredOpen unproven-1811
WorkGraphLifecycleMachine Open input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindOpen ClassifyConfirmationAdmissionSelfAttestEmptyOpen unproven-1811
WorkGraphLifecycleMachine Open input:ClassifyConfirmationAdmission ClassifyConfirmationAdmissionEvidenceKindOpen ClassifyConfirmationAdmissionSupervisorMismatchOpen unproven-1811
WorkGraphLifecycleMachine Open input:PolicyEscalate PolicyEscalateOpenAdmitted PolicyEscalateOpenDenied unproven-1811
";
