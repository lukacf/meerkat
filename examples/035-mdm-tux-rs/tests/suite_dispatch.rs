//! The live suites must not teach a made-up typed peer request. Check their
//! operator prompts without requiring tmux, a provider, or a running target.

#[test]
fn suite_prompts_do_not_dispatch_through_execute_task() {
    for (suite, script) in [
        ("live", include_str!("../scripts/tmux-live-suite.sh")),
        (
            "architecture",
            include_str!("../scripts/tmux-architecture-suite.sh"),
        ),
    ] {
        let prompts: Vec<_> = script
            .lines()
            .filter(|line| line.trim_start().starts_with("send_prompt "))
            .collect();
        assert!(!prompts.is_empty(), "{suite}: no operator prompts found");
        for prompt in prompts {
            assert!(
                !prompt.contains("execute_task"),
                "{suite}: execute_task is not a supported peer request: {prompt}"
            );
        }
    }
}
