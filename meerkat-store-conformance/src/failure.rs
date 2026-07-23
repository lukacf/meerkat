//! Typed conformance failure with chapter/step context.

use std::fmt::Display;

/// One conformance-suite failure with enough context for downstream CI
/// output to be actionable: the chapter that failed, the step inside it,
/// and a human-readable detail describing the violated contract.
#[derive(Debug, thiserror::Error)]
#[error("storage conformance failure [{chapter}/{step}]: {detail}")]
pub struct ConformanceFailure {
    chapter: String,
    step: String,
    detail: String,
}

impl ConformanceFailure {
    /// Construct a failure. Downstream factories and custom chapters use
    /// this too (e.g. `ConformanceFailure::new("factory", "open", ...)`).
    pub fn new(
        chapter: impl Into<String>,
        step: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            chapter: chapter.into(),
            step: step.into(),
            detail: detail.into(),
        }
    }

    /// The chapter that failed (e.g. `"baseline"`).
    pub fn chapter(&self) -> &str {
        &self.chapter
    }

    /// The step inside the chapter (e.g. `"delete_if_current_revision_guard"`).
    pub fn step(&self) -> &str {
        &self.step
    }

    /// Human-readable description of the violated contract.
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// Internal per-chapter step-context helper.
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
