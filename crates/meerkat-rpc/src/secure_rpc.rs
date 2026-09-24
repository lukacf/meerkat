//! Conservative TCP admission policy for the JSON-RPC host.
//!
//! This module owns only transport-listener safety checks. It does not decide
//! who may perform RPC actions once connected; that belongs to auth/grants.

use std::net::{IpAddr, ToSocketAddrs};

/// Policy requested by the process owner for TCP listeners.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpBindPolicy {
    pub allow_remote: bool,
}

impl TcpBindPolicy {
    pub const fn local_only() -> Self {
        Self {
            allow_remote: false,
        }
    }

    pub const fn allow_remote() -> Self {
        Self { allow_remote: true }
    }
}

/// Reason a requested TCP bind is refused before opening the listener.
#[derive(Debug, thiserror::Error)]
pub enum TcpBindPolicyError {
    #[error("could not resolve TCP bind address `{addr}`: {source}")]
    Resolve {
        addr: String,
        #[source]
        source: std::io::Error,
    },
    #[error("TCP bind address `{addr}` resolved to no socket addresses")]
    EmptyResolution { addr: String },
    #[error(
        "{surface} TCP bind address `{addr}` resolves to non-loopback address `{ip}`; pass --allow-remote only when an authenticated/encrypted transport wrapper is in place"
    )]
    RemoteBindRequiresExplicitAllow {
        surface: &'static str,
        addr: String,
        ip: IpAddr,
    },
}

/// Enforce local-only TCP listeners unless the process owner opts in.
pub fn validate_tcp_bind_policy(
    surface: &'static str,
    addr: &str,
    policy: TcpBindPolicy,
) -> Result<(), TcpBindPolicyError> {
    let resolved = addr
        .to_socket_addrs()
        .map_err(|source| TcpBindPolicyError::Resolve {
            addr: addr.to_string(),
            source,
        })?
        .collect::<Vec<_>>();

    if resolved.is_empty() {
        return Err(TcpBindPolicyError::EmptyResolution {
            addr: addr.to_string(),
        });
    }

    if policy.allow_remote {
        return Ok(());
    }

    for socket_addr in resolved {
        let ip = socket_addr.ip();
        if !ip.is_loopback() {
            return Err(TcpBindPolicyError::RemoteBindRequiresExplicitAllow {
                surface,
                addr: addr.to_string(),
                ip,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn local_only_policy_accepts_loopback_ipv4_and_ipv6() {
        validate_tcp_bind_policy("rpc", "127.0.0.1:4800", TcpBindPolicy::local_only())
            .expect("ipv4 loopback is local-only");
        validate_tcp_bind_policy("rpc", "[::1]:4800", TcpBindPolicy::local_only())
            .expect("ipv6 loopback is local-only");
    }

    #[test]
    fn local_only_policy_rejects_wildcard_binds() {
        let err = validate_tcp_bind_policy("rpc", "0.0.0.0:4800", TcpBindPolicy::local_only())
            .expect_err("wildcard bind must be refused without explicit opt-in");

        assert!(matches!(
            err,
            TcpBindPolicyError::RemoteBindRequiresExplicitAllow { ip, .. }
                if ip == IpAddr::from([0, 0, 0, 0])
        ));
    }

    #[test]
    fn explicit_remote_policy_accepts_wildcard_binds() {
        validate_tcp_bind_policy("rpc", "0.0.0.0:4800", TcpBindPolicy::allow_remote())
            .expect("explicit remote opt-in permits non-loopback bind");
    }

    #[test]
    fn local_only_policy_rejects_non_loopback_addresses() {
        let err = validate_tcp_bind_policy("rpc", "192.0.2.10:4800", TcpBindPolicy::local_only())
            .expect_err("non-loopback bind must be refused without explicit opt-in");

        assert!(matches!(
            err,
            TcpBindPolicyError::RemoteBindRequiresExplicitAllow { ip, .. }
                if ip == IpAddr::from([192, 0, 2, 10])
        ));
    }
}
