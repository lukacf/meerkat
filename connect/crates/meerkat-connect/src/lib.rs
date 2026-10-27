//! Native setup conversation shared by Connect presenters.
//!
//! The native machine owns admission, turns, cancellation and persistence.
//! Presenters supply complete messages and consume native events/outcomes.
//! A completed conversation turn is not an installation or grant receipt.

pub mod approval;
pub mod application_tool_policy;
#[cfg(unix)]
pub mod application_operator;
#[cfg(all(unix, feature = "console"))]
pub mod application_gateway;
#[cfg(all(feature = "console", unix))]
pub mod application_remote;
#[cfg(all(feature = "console", unix))]
pub mod application_bootstrap;
#[cfg(all(feature = "console", unix))]
pub mod application_setup;
#[cfg(all(feature = "console", unix))]
pub mod application_acquisition;
#[cfg(all(feature = "console", unix))]
pub mod application_driver;
#[cfg(all(feature = "console", unix))]
pub mod application_update;
mod connector_auth;
mod connector_install;
pub mod installed_service_review;
#[cfg(feature = "console")]
pub mod console;
mod frontend_catalog;
pub mod forge;
pub mod paired_host;
pub mod paired_host_device;
pub mod paired_host_tool;
pub mod host_selection;
mod paired_host_stdio;
pub mod pairing;
pub mod progress;
pub mod provider_setup;
mod public_document;
pub mod realm;
mod recipe_discovery;
pub mod research;
pub mod resources;
pub mod secret_presenter;
mod service_install;
pub mod service_preparation;
pub mod session;
pub mod stdio;
pub mod setup;
pub mod terminal;
pub mod terminal_connector;

pub use realm::{RealmChoice, RealmOptions, discover_realms};
pub use recipe_discovery::{RECIPE_TOOLS, RecipeTools};
pub use session::{ConnectSession, SessionOptions, SessionStartup};
pub use paired_host_tool::{
    PAIRED_HOST_PREPARATION_INSTRUCTIONS, PAIRED_HOST_PREPARATION_TOOLS, PairedHostPreparationTools,
};

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// Request-local cancellation. A prior decision or entered native effect
    /// remains owned and joined; this variant makes no claim that it did not occur.
    #[error("The account review request was cancelled")]
    ReviewCancelled,
    /// Provider errors retain their typed source for embedding hosts. Their
    /// display intentionally excludes provider bodies and private OAuth data.
    #[error("{operation} failed; provider details are withheld from terminal output")]
    ProviderSetup {
        operation: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("{operation}: {source}")]
    Native {
        operation: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    #[error("{0}")]
    InvalidInput(String),
    #[error(
        "native input admission did not return a completion handle; do not retry automatically"
    )]
    MissingCompletion {
        admission: Box<meerkat_runtime::AcceptOutcome>,
    },
    #[error("native session materialization did not retain its registration")]
    MissingRegistration,
    #[error("native session materialization did not provide its executor attachment")]
    MissingAttachment,
    #[error("{primary}; cleanup also failed: {cleanup}")]
    Cleanup {
        primary: Box<ConnectError>,
        cleanup: Box<ConnectError>,
    },
}

impl ConnectError {
    pub(crate) fn provider_setup(
        operation: &'static str,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::ProviderSetup { operation, source: Box::new(source) }
    }
    pub fn native(
        operation: &'static str,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::Native {
            operation,
            source: Box::new(source),
        }
    }
}

// Compile the exact private CLI cancellation route in native regression controls.
#[cfg(test)]
mod main_connector;
#[cfg(test)]
extern crate self as meerkat_connect;
