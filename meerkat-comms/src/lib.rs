// meerkat-comms
//! Inter-agent communication for Meerkat instances.

pub mod identity;
pub mod trust;
pub mod types;

pub use identity::{IdentityError, Keypair, PubKey, Signature};
pub use trust::{TrustedPeer, TrustedPeers};
pub use types::{Envelope, InboxItem, MessageKind, Status};
