use meerkat_core::generated::protocol_ops_barrier_satisfaction::accept_wait_all_satisfied;

struct RawEffectShape {
    wait_request_id: String,
    operation_ids: Vec<String>,
}

fn main() {
    let raw = RawEffectShape {
        wait_request_id: "wait-1".into(),
        operation_ids: vec!["op-1".into()],
    };
    let _ = accept_wait_all_satisfied(raw);
}
