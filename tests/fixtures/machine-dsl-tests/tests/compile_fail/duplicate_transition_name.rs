use meerkat_machine_dsl::machine;

machine! {
    machine Bad {
        version: 1,
        rust: "test" / "bad",

        state {
            phase: BadPhase,
        }

        init(On) {}

        terminal []

        phase BadPhase {
            On,
            Off,
        }

        input BadInput {
            Toggle,
        }

        transition DoThing {
            on input Toggle
            guard { self.phase == Phase::On }
            update {}
            to Off
        }

        transition DoThing {
            on input Toggle
            guard { self.phase == Phase::Off }
            update {}
            to On
        }
    }
}

fn main() {}
