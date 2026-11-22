//! Which host names the REST server answers for.
//!
//! A browser sends the name a page used to reach a server in the `Host`
//! header. A page can make its own name resolve to this server's address
//! and then read the server's answers as its own. So the server answers only
//! for a name that is its own: `localhost`, an address literal, or a name
//! the realm's `[rest] served_hosts` lists, whatever address it is bound to.
//! A reverse proxy that forwards its public name, and a caller that uses a
//! container or service name, are such callers: an operator lists that name.
//! The refusal names the host it refused, so the operator sees what to list.
//!
//! A request without a `Host` header (no browser sends one) is served.

use std::net::IpAddr;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderMap, StatusCode, Uri, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::ErrorResponse;

/// The `code` of the refusal of a request that names a host this server
/// does not answer for.
pub const HOST_NOT_SERVED: &str = "HOST_NOT_SERVED";

/// The longest host name the refusal names: the longest DNS name.
const NAMED_HOST_MAX_CHARS: usize = 253;

/// The host names this server answers for: `localhost`, address literals
/// and the realm's `[rest] served_hosts`, read at startup.
#[derive(Debug, Clone)]
pub struct ServedHosts {
    /// Whether the server listens on a loopback address (`[rest] host`).
    bound_to_loopback: bool,
    /// The listed names, as [`host_name`] writes a name.
    listed: Arc<[String]>,
}

impl Default for ServedHosts {
    /// A server on loopback whose realm lists no name.
    fn default() -> Self {
        Self {
            bound_to_loopback: true,
            listed: Arc::from([]),
        }
    }
}

impl ServedHosts {
    /// The names a server bound to `bound_host` (`[rest] host`) answers for,
    /// with the names the realm lists. `Config::validate` has refused every
    /// entry that is not one host name.
    pub fn new(bound_host: &str, served_hosts: &[String]) -> Self {
        let bound_to_loopback = bound_host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
        Self {
            bound_to_loopback,
            listed: served_hosts.iter().map(|name| host_name(name)).collect(),
        }
    }

    /// Whether this server can be reached from other machines while its
    /// realm lists no name: every caller that addresses it by a name is then
    /// refused. The binary says so when it starts.
    pub fn refuses_every_name_off_loopback(&self) -> bool {
        !self.bound_to_loopback && self.listed.is_empty()
    }

    /// Whether a request addressed to `authority` (a `Host` header value)
    /// is addressed to this server. Ports do not matter: a page that makes
    /// its own name resolve to this server still sends that name.
    fn serves(&self, authority: &str) -> bool {
        let name = host_name(authority);
        name == "localhost" || name.parse::<IpAddr>().is_ok() || self.listed.contains(&name)
    }

    /// The host a request with these headers and this target names that
    /// this server does not answer for, when it names one.
    fn refused_host(&self, headers: &HeaderMap, target: &Uri) -> Option<String> {
        let mut hosts = headers.get_all(header::HOST).iter();
        let first = hosts.next();
        if let Some(host) = first {
            let served = host.to_str().is_ok_and(|host| self.serves(host));
            // A request names one host.
            if !served || hosts.next().is_some() {
                return Some(named_host(host.as_bytes()));
            }
        }
        target
            .authority()
            .filter(|authority| !self.serves(authority.as_str()))
            .map(|authority| named_host(authority.as_str().as_bytes()))
    }
}

/// The host name of a `Host` header value as the check compares it: without
/// its port, the brackets of an IPv6 literal or a trailing dot, lowercased.
fn host_name(authority: &str) -> String {
    let authority = authority.trim();
    let name = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        // An address literal without a port (an IPv6 one holds colons).
        None if authority.parse::<IpAddr>().is_ok() => authority,
        None => match authority.rsplit_once(':') {
            Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
            _ => authority,
        },
    };
    name.trim_end_matches('.').to_ascii_lowercase()
}

/// A refused host as the refusal names it: the name as the check compares
/// it, without control characters, at most [`NAMED_HOST_MAX_CHARS`] long.
fn named_host(authority: &[u8]) -> String {
    host_name(&String::from_utf8_lossy(authority))
        .chars()
        .filter(|c| !c.is_control())
        .take(NAMED_HOST_MAX_CHARS)
        .collect()
}

/// Refuse a request that names a host this server does not answer for,
/// before any route runs: `403` in the server's error envelope, with the
/// refused host in the message and in `details.host`.
pub(crate) async fn refuse_host_not_served(
    State(served): State<ServedHosts>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(host) = served.refused_host(request.headers(), request.uri()) {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: format!(
                    "this server does not answer for the host `{host}`; an operator lists a \
                     name the server is reached under in the realm's `[rest] served_hosts`"
                ),
                code: HOST_NOT_SERVED.to_string(),
                details: Some(json!({ "host": host })),
            }),
        )
            .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The startup notice's condition: off loopback with no listed name.
    #[test]
    fn a_server_refuses_every_name_only_off_loopback_with_no_listed_name() {
        for bound in ["0.0.0.0", "::", "[::]", "192.168.1.5"] {
            assert!(
                ServedHosts::new(bound, &[]).refuses_every_name_off_loopback(),
                "{bound}"
            );
            assert!(
                !ServedHosts::new(bound, &["api.example.com".to_string()])
                    .refuses_every_name_off_loopback(),
                "{bound} with a listed name"
            );
        }
        for bound in ["127.0.0.1", "127.0.0.2", "::1", "[::1]"] {
            assert!(
                !ServedHosts::new(bound, &[]).refuses_every_name_off_loopback(),
                "{bound}"
            );
        }
        assert!(!ServedHosts::default().refuses_every_name_off_loopback());
    }

    #[test]
    fn a_host_name_is_compared_without_port_brackets_case_or_trailing_dot() {
        for (authority, name) in [
            ("localhost", "localhost"),
            ("LocalHost.:8080", "localhost"),
            ("api.example.com:443", "api.example.com"),
            ("127.0.0.1:8080", "127.0.0.1"),
            ("[::1]:8080", "::1"),
            ("[::1]", "::1"),
            ("::1", "::1"),
            ("fd00::7", "fd00::7"),
        ] {
            assert_eq!(host_name(authority), name, "{authority}");
        }
    }

    #[test]
    fn a_refused_host_is_named_bounded_and_without_control_characters() {
        assert_eq!(named_host(b"Api.Example.com:8443"), "api.example.com");
        assert_eq!(named_host(b"api\t.exa\x7fmple.com"), "api.example.com");
        assert_eq!(named_host(&[b'a'; 400]).len(), NAMED_HOST_MAX_CHARS);
        // Bytes that are not text are named by the replacement character.
        assert_eq!(named_host(b"caf\xff.example"), "caf\u{fffd}.example");
    }
}
