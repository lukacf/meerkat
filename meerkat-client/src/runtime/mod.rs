//! Provider-runtime layer: typed validation, resolution, and client
//! construction.

pub mod binding;
pub mod errors;
pub mod provider_runtime;
pub mod registry;
pub mod resolver;

pub use binding::{
    DynamicLease, NormalizedAuthMethod, NormalizedBackendKind, ResolvedConnection, StaticLease,
    ValidatedBinding,
};
pub use errors::{ProviderAuthError, ProviderBindingError, ProviderClientError};
pub use provider_runtime::ProviderRuntime;
pub use registry::{ExternalAuthResolverHandle, ProviderRuntimeRegistry, ResolverEnvironment};
