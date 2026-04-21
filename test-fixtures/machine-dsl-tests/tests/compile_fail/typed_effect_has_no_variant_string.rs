use meerkat_machine_kernels::generated::meerkat::{
    transition, EmptyContext, Input, State,
};
use meerkat_machine_schema::catalog::dsl::meerkat_machine::{
    AgentRuntimeId, FenceToken, Generation,
};

fn main() {
    let state = State::default();
    let input = Input::PrepareBindings {
        agent_runtime_id: AgentRuntimeId::from("runtime-1"),
        fence_token: FenceToken::from("fence-1"),
        generation: Generation::from("generation-0"),
    };
    let outcome = transition(&state, input, &EmptyContext).unwrap();

    let _ = outcome.effects.first().map(|effect| effect.variant.as_str());
}
