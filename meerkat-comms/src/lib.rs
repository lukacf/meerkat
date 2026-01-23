// meerkat-comms
//! Inter-agent communication for Meerkat instances.

pub mod identity;
pub mod inbox;
pub mod io_task;
pub mod router;
pub mod transport;
pub mod trust;
pub mod types;

pub use identity::{IdentityError, Keypair, PubKey, Signature};
pub use inbox::{Inbox, InboxError, InboxSender};
pub use io_task::{handle_connection, IoTaskError};
pub use router::{CommsConfig, Router, SendError, DEFAULT_MAX_MESSAGE_BYTES};
pub use transport::{PeerAddr, TransportError};
pub use trust::{TrustError, TrustedPeer, TrustedPeers};
pub use types::{Envelope, InboxItem, MessageKind, Status};
