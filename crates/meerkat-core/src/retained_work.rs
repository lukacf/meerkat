//! Identity of the retained work a run performs, as passive contract data.
//!
//! A [`RetainedWorkIdentity`] names one staged run exactly: the runtime and
//! run, the retained original contributors in staged order, the selected-row
//! bindings and the controller selection. It proves identity equality only.
//! It is never an issuance, a permission or a runnable client: the runtime
//! mints it from the batch it actually staged, and a later resume of that
//! work is admitted only with owner-issued custody and a current permission
//! check, never on the strength of this data.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ControllerModelSelection;
use crate::lifecycle::{InputId, RunId};

/// One retained original contributor of a staged run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedContributorRef {
    /// The contributing input's id in the runtime that staged the run.
    pub input_id: InputId,
    /// Hex digest of the contributor's retained authority association.
    pub association_digest: String,
    /// Hex digest of the contributor's exact submitted input (its retained
    /// replay digest).
    pub submission_digest: String,
}

/// The canonical binding and batch key of one selected row of a staged run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedInputBinding {
    pub binding: String,
    pub batch_key: String,
}

/// Exact identity of one staged run's retained work. Equality only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetainedWorkIdentity {
    runtime_id: String,
    run_id: RunId,
    contributors: Vec<RetainedContributorRef>,
    selected_input_bindings: BTreeMap<String, SelectedInputBinding>,
    #[serde(deserialize_with = "Option::deserialize")]
    controller: Option<ControllerModelSelection>,
}

impl RetainedWorkIdentity {
    /// Describe a staged run. Called by the runtime from the batch it staged;
    /// constructing one grants nothing.
    pub fn new(
        runtime_id: impl Into<String>,
        run_id: RunId,
        contributors: Vec<RetainedContributorRef>,
        selected_input_bindings: BTreeMap<String, SelectedInputBinding>,
        controller: Option<ControllerModelSelection>,
    ) -> Self {
        Self {
            runtime_id: runtime_id.into(),
            run_id,
            contributors,
            selected_input_bindings,
            controller,
        }
    }

    pub fn runtime_id(&self) -> &str {
        &self.runtime_id
    }

    pub fn run_id(&self) -> &RunId {
        &self.run_id
    }

    /// The retained original contributors, in staged order.
    pub fn contributors(&self) -> &[RetainedContributorRef] {
        &self.contributors
    }

    pub fn selected_input_bindings(&self) -> &BTreeMap<String, SelectedInputBinding> {
        &self.selected_input_bindings
    }

    /// The controller selection as data; never a runnable client.
    pub fn controller(&self) -> Option<&ControllerModelSelection> {
        self.controller.as_ref()
    }
}
