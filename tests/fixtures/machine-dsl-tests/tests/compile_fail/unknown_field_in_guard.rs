use meerkat_machine_dsl::machine;

machine! {
    machine Bad {
        version: 1,
        rust: "test" / "bad",

        state {
            active: bool,
        }

        init(On) {
            active = true,
        }

        terminal []

        phase BadPhase {
            On,
            Off,
        }

        input BadInput {
            Toggle,
        }

        transition ToggleBad {
            on input Toggle
            guard { self.nonexistent_field == true }
            update {}
            to Off
        }
    }
}

fn main() {}
