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

        terminal [NonexistentPhase]

        phase BadPhase {
            On,
            Off,
        }

        input BadInput {
            Toggle,
        }

        transition ToggleBad {
            on input Toggle
            guard { self.active }
            update { self.active = false; }
            to Off
        }
    }
}

fn main() {}
