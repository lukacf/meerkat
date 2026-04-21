#![allow(clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};

use meerkat_machine_schema::{
    Expr, MachineSchema, canonical_machine_schemas, flow_frame_machine, flow_run_machine,
    loop_iteration_machine,
};

fn row22_machine_schemas() -> Vec<MachineSchema> {
    let mut schemas = canonical_machine_schemas();
    schemas.extend([
        flow_run_machine(),
        flow_frame_machine(),
        loop_iteration_machine(),
    ]);
    schemas
}

fn collect_helper_calls(expr: &Expr, calls: &mut BTreeSet<String>) {
    match expr {
        Expr::Bool(_)
        | Expr::U64(_)
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
        Expr::SeqLiteral(items) | Expr::And(items) | Expr::Or(items) => {
            for item in items {
                collect_helper_calls(item, calls);
            }
        }
        Expr::IfElse {
            condition,
            then_expr,
            else_expr,
        } => {
            collect_helper_calls(condition, calls);
            collect_helper_calls(then_expr, calls);
            collect_helper_calls(else_expr, calls);
        }
        Expr::Not(expr)
        | Expr::Len(expr)
        | Expr::Head(expr)
        | Expr::MapKeys(expr)
        | Expr::SeqElements(expr)
        | Expr::Some(expr) => collect_helper_calls(expr, calls),
        Expr::Eq(left, right)
        | Expr::Neq(left, right)
        | Expr::Add(left, right)
        | Expr::Sub(left, right)
        | Expr::Gt(left, right)
        | Expr::Gte(left, right)
        | Expr::Lt(left, right)
        | Expr::Lte(left, right) => {
            collect_helper_calls(left, calls);
            collect_helper_calls(right, calls);
        }
        Expr::Contains { collection, value } => {
            collect_helper_calls(collection, calls);
            collect_helper_calls(value, calls);
        }
        Expr::MapContainsKey { map, key } | Expr::MapGet { map, key } => {
            collect_helper_calls(map, calls);
            collect_helper_calls(key, calls);
        }
        Expr::SeqStartsWith { seq, prefix } => {
            collect_helper_calls(seq, calls);
            collect_helper_calls(prefix, calls);
        }
        Expr::Call { helper, args } => {
            calls.insert(helper.clone());
            for arg in args {
                collect_helper_calls(arg, calls);
            }
        }
        Expr::Quantified { over, body, .. } => {
            collect_helper_calls(over, calls);
            collect_helper_calls(body, calls);
        }
    }
}

fn helper_graph(schema: &MachineSchema) -> BTreeMap<String, BTreeSet<String>> {
    let known = schema
        .helpers
        .iter()
        .chain(schema.derived.iter())
        .map(|helper| helper.name.clone())
        .collect::<BTreeSet<_>>();

    schema
        .helpers
        .iter()
        .chain(schema.derived.iter())
        .map(|helper| {
            let mut deps = BTreeSet::new();
            collect_helper_calls(&helper.body, &mut deps);
            let deps = deps
                .into_iter()
                .filter(|dep| dep != &helper.name && known.contains(dep))
                .collect::<BTreeSet<_>>();
            (helper.name.clone(), deps)
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Done,
}

fn detect_helper_cycle(schema: &MachineSchema) -> Option<Vec<String>> {
    fn visit(
        node: &str,
        graph: &BTreeMap<String, BTreeSet<String>>,
        state: &mut BTreeMap<String, VisitState>,
        stack: &mut Vec<String>,
    ) -> Option<Vec<String>> {
        match state.get(node).copied() {
            Some(VisitState::Done) => return None,
            Some(VisitState::Visiting) => {
                let start = stack
                    .iter()
                    .position(|entry| entry == node)
                    .expect("cycle node should be present in stack");
                let mut cycle = stack[start..].to_vec();
                cycle.push(node.to_string());
                return Some(cycle);
            }
            None => {}
        }

        state.insert(node.to_string(), VisitState::Visiting);
        stack.push(node.to_string());
        for dep in graph.get(node).into_iter().flatten() {
            if let Some(cycle) = visit(dep, graph, state, stack) {
                return Some(cycle);
            }
        }
        stack.pop();
        state.insert(node.to_string(), VisitState::Done);
        None
    }

    let graph = helper_graph(schema);
    let mut state = BTreeMap::new();
    let mut stack = Vec::new();
    for node in graph.keys() {
        if let Some(cycle) = visit(node, &graph, &mut state, &mut stack) {
            return Some(cycle);
        }
    }
    None
}

#[test]
fn canonical_and_compat_helper_graphs_are_acyclic() {
    for schema in row22_machine_schemas() {
        let cycle = detect_helper_cycle(&schema);
        assert!(
            cycle.is_none(),
            "{} helper graph should be acyclic, got {cycle:?}",
            schema.machine
        );
    }
}

#[test]
fn helper_call_cycles_are_rejected_by_validation_coverage() {
    let mut schema = canonical_machine_schemas()
        .into_iter()
        .find(|schema| schema.machine == "MeerkatMachine")
        .expect("MeerkatMachine schema");

    schema.helpers = vec![
        meerkat_machine_schema::HelperSchema {
            name: "helper_a".into(),
            params: vec![],
            returns: meerkat_machine_schema::TypeRef::Bool,
            body: Expr::Call {
                helper: "helper_b".into(),
                args: vec![],
            },
        },
        meerkat_machine_schema::HelperSchema {
            name: "helper_b".into(),
            params: vec![],
            returns: meerkat_machine_schema::TypeRef::Bool,
            body: Expr::Call {
                helper: "helper_a".into(),
                args: vec![],
            },
        },
    ];
    schema.derived.clear();

    let cycle = detect_helper_cycle(&schema).expect("helper cycle");
    assert!(
        cycle
            .windows(2)
            .any(|pair| pair[0] == "helper_a" && pair[1] == "helper_b")
            || cycle
                .windows(2)
                .any(|pair| pair[0] == "helper_b" && pair[1] == "helper_a"),
        "expected synthetic cycle to mention helper_a/helper_b, got {cycle:?}"
    );
}
