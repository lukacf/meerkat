//! Per-chapter step-context helper mirroring the upstream harness's
//! (crate-private) `Steps` type over the shared [`ConformanceFailure`].

use std::fmt::Display;

use meerkat_store_conformance::ConformanceFailure;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Steps {
    chapter: &'static str,
}

impl Steps {
    pub(crate) fn chapter(chapter: &'static str) -> Self {
        Self { chapter }
    }

    pub(crate) fn fail(&self, step: &'static str, detail: impl Into<String>) -> ConformanceFailure {
        ConformanceFailure::new(self.chapter, step, detail)
    }

    /// Map an arbitrary error into a step-scoped failure.
    pub(crate) fn wrap<T, E: Display>(
        &self,
        step: &'static str,
        result: Result<T, E>,
    ) -> Result<T, ConformanceFailure> {
        result.map_err(|error| self.fail(step, error.to_string()))
    }

    /// Assert a condition with a step-scoped failure.
    pub(crate) fn ensure(
        &self,
        step: &'static str,
        condition: bool,
        detail: impl Into<String>,
    ) -> Result<(), ConformanceFailure> {
        if condition {
            Ok(())
        } else {
            Err(self.fail(step, detail))
        }
    }
}
