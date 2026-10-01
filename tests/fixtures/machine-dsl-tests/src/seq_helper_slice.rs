//! Helpers with sequence parameters.
//!
//! A `Seq<T>` helper parameter expands to `&[T]`, not `&Vec<T>` (which
//! clippy's `ptr_arg` rejects under the workspace lints), and callers passing
//! a state field or an input binding still coerce to it.
use meerkat_machine_dsl::machine;

machine! {
    machine SeqHelperSlice {
        version: 1,
        rust: "machine-dsl-tests" / "seq_helper_slice",

        state {
            lifecycle_phase: SeqPhase,
            history: Seq<String>,
        }

        init(Open) {
            history = EmptySeq,
        }

        terminal []

        phase SeqPhase {
            Open,
        }

        input SeqInput {
            Check { chain: Seq<String>, entry: String },
        }

        effect SeqEffect {
            Checked,
        }

        disposition Checked => local seam NoOwnerRealization,

        helper chain_bounded(chain: Seq<String>, limit: u64) -> bool {
            chain.len() <= limit
        }

        helper chain_holds(chain: Seq<String>, entry: String) -> bool {
            chain.contains(entry) && chain_bounded(chain, 4)
        }

        transition CheckChain {
            on input Check { chain, entry }
            guard "bounded" { chain_bounded(self.history, 4) && chain_holds(chain, entry) }
            update {}
            to Open
            emit Checked
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(chain: &[&str], entry: &str) -> bool {
        let mut auth = SeqHelperSliceAuthority::new();
        SeqHelperSliceMutator::apply(
            &mut auth,
            SeqInput::Check {
                chain: chain.iter().map(|link| (*link).to_owned()).collect(),
                entry: entry.to_owned(),
            },
        )
        .is_ok()
    }

    #[test]
    fn seq_helpers_take_slices_and_evaluate_on_fields_and_bindings() {
        assert!(check(&["a", "b"], "b"));
        assert!(!check(&["a", "b"], "c"), "entry must be in the chain");
        assert!(
            !check(&["a", "b", "c", "d", "e"], "a"),
            "chain longer than the bound must be rejected"
        );
    }

    #[test]
    fn seq_helper_schema_is_valid() {
        SeqHelperSliceState::schema()
            .validate()
            .expect("schema should be valid");
    }
}
