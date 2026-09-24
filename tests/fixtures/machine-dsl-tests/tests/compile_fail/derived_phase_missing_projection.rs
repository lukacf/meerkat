use meerkat_machine_dsl::machine;

machine! {
    machine Bad {
        version: 1,
        rust: "test" / "bad",

        state {
            value: u64,
        }

        init(Low) {
            value = 0,
        }

        terminal []

        phase BadPhase {
            Low,
            High,
        }

        // No phase_projection block — should error for derived-phase machine

        input BadInput {
            Bump,
        }

        transition BumpIt {
            on input Bump
            update { self.value += 1; }
            to High
        }
    }
}

fn main() {}
