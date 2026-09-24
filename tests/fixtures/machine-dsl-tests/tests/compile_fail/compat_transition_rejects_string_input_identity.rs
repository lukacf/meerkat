use meerkat_machine_kernels::generated::flow_run;

fn main() {
    let state = flow_run::initial_state().unwrap();
    let _ = flow_run::transition(&state, "StartRun".to_string(), &flow_run::EmptyContext);
}
