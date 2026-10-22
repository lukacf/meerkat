//! Host-owned HTTP egress boundary for native MCP transports and their
//! credential owner.
//!
//! A host that must own where native HTTP traffic goes selects an
//! [`HttpEgressProfile`]. [`HttpEgressProfile::trusted_host`] keeps the
//! existing behavior. A required profile is compiled once from an exact
//! [`HttpEgressPolicy`] and is applied by the native clients themselves:
//!
//! - every request URI is admitted against an exact scheme, host and
//!   effective port before it is sent (an explicit URL port overrides any
//!   port a resolver returns, so the port is decided here);
//! - the clients' own DNS resolver admits every resolved address of an
//!   admitted host name (an IP-literal host never reaches a resolver, so its
//!   address is admitted with the URI);
//! - no redirect is followed: every 3xx answer is returned to the caller,
//!   which refuses it by its status (so a resolver refusal is always the
//!   request's own first and only hop);
//! - no ambient (environment or system) proxy is used.
//!
//! This is an execution boundary, not a permission: native permission,
//! account and currentness checks still decide whether a request is made at
//! all. Admission reads immutable compiled data and holds no lock.
//!
//! `localhost` is not special. It is a host name like any other: it must be
//! named by an origin, and it resolves through the system resolver, so its
//! answer (commonly both `127.0.0.1` and `::1`) is admitted only as
//! deliberate local authority listing exactly those addresses
//! ([`HttpEgressPolicy::allow_local`]). A public-only destination never
//! reaches loopback, and an answer that mixes admitted and refused addresses
//! is refused whole.

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use reqwest::Url;

/// Scheme of an exact egress origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HttpEgressScheme {
    Http,
    Https,
}

impl HttpEgressScheme {
    fn of(url: &Url) -> Option<Self> {
        match url.scheme() {
            "http" => Some(Self::Http),
            "https" => Some(Self::Https),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

/// The host of an egress origin, normalized exactly as request URLs are
/// parsed (lowercase ASCII domain, or canonical IP literal).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum EgressHost {
    Domain(String),
    Ip(IpAddr),
}

impl EgressHost {
    fn of(url: &Url) -> Option<Self> {
        let host = url.host_str()?;
        if let Some(literal) = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
        {
            return literal
                .parse::<Ipv6Addr>()
                .ok()
                .map(|ip| Self::Ip(canonical_ip(IpAddr::V6(ip))));
        }
        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            return Some(Self::Ip(IpAddr::V4(ip)));
        }
        Some(Self::Domain(host.to_owned()))
    }
}

impl std::fmt::Display for EgressHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Domain(domain) => f.write_str(domain),
            Self::Ip(IpAddr::V4(ip)) => write!(f, "{ip}"),
            Self::Ip(IpAddr::V6(ip)) => write!(f, "[{ip}]"),
        }
    }
}

/// One exact HTTP origin: scheme, host and effective port (the explicit port,
/// or the scheme's default).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HttpEgressOrigin {
    scheme: HttpEgressScheme,
    host: EgressHost,
    port: u16,
}

/// An origin string that is not an exact `http` or `https` origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid HTTP egress origin: expected scheme://host[:port] with no credentials, path, query or fragment"
)]
pub struct HttpEgressOriginError;

impl HttpEgressOrigin {
    /// Parse an exact origin such as `https://api.example.com` or
    /// `http://127.0.0.1:8931`. Userinfo, a query, a fragment and any path
    /// other than `/` are refused: an origin names a destination, nothing
    /// else.
    pub fn parse(origin: &str) -> Result<Self, HttpEgressOriginError> {
        let url = Url::parse(origin).map_err(|_| HttpEgressOriginError)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err(HttpEgressOriginError);
        }
        Self::of(&url).ok_or(HttpEgressOriginError)
    }

    fn of(url: &Url) -> Option<Self> {
        Some(Self {
            scheme: HttpEgressScheme::of(url)?,
            host: EgressHost::of(url)?,
            port: url.port_or_known_default()?,
        })
    }

    pub fn scheme(&self) -> HttpEgressScheme {
        self.scheme
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl std::fmt::Display for HttpEgressOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}://{}:{}", self.scheme.as_str(), self.host, self.port)
    }
}

/// The addresses an admitted host may resolve to (or, for an IP-literal
/// host, may be).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpEgressAddresses {
    /// Globally routable unicast addresses only. Loopback, private,
    /// link-local, unique-local, shared (CGNAT), multicast, documentation,
    /// benchmarking, unspecified and reserved ranges are refused, including
    /// when embedded in an IPv4-mapped or NAT64 IPv6 address.
    Public,
    /// Deliberate host authority for a local or private service: exactly
    /// these addresses, of any class (loopback included). Nothing is implied
    /// beyond the listed set.
    Exactly(BTreeSet<IpAddr>),
}

impl HttpEgressAddresses {
    fn admits(&self, ip: IpAddr) -> bool {
        let ip = canonical_ip(ip);
        match self {
            Self::Public => is_public_unicast(ip),
            Self::Exactly(addresses) => addresses.contains(&ip),
        }
    }
}

/// The host's exact destination policy. It is not a permission language:
/// each entry names one origin and the addresses its host may use.
#[derive(Debug, Clone, Default)]
pub struct HttpEgressPolicy {
    destinations: Vec<(HttpEgressOrigin, HttpEgressAddresses)>,
}

impl HttpEgressPolicy {
    /// A policy that admits nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Admit `origin`, whose host may resolve only to public addresses.
    #[must_use]
    pub fn allow_public(mut self, origin: HttpEgressOrigin) -> Self {
        self.destinations
            .push((origin, HttpEgressAddresses::Public));
        self
    }

    /// Admit `origin` as a deliberately selected local or private service
    /// whose host may resolve only to exactly `addresses`. Loopback is never
    /// implied: it must be listed.
    #[must_use]
    pub fn allow_local(
        mut self,
        origin: HttpEgressOrigin,
        addresses: impl IntoIterator<Item = IpAddr>,
    ) -> Self {
        let addresses = addresses.into_iter().map(canonical_ip).collect();
        self.destinations
            .push((origin, HttpEgressAddresses::Exactly(addresses)));
        self
    }

    fn compile(self) -> Result<EgressRules, HttpEgressUnavailable> {
        let mut origins = BTreeSet::new();
        let mut hosts: BTreeMap<EgressHost, HttpEgressAddresses> = BTreeMap::new();
        for (origin, addresses) in self.destinations {
            if matches!(&addresses, HttpEgressAddresses::Exactly(set) if set.is_empty()) {
                return Err(HttpEgressUnavailable::InvalidPolicy);
            }
            // The resolver sees only a host name, so one host has exactly one
            // address set whatever the scheme or port.
            match hosts.get(&origin.host) {
                Some(existing) if existing != &addresses => {
                    return Err(HttpEgressUnavailable::InvalidPolicy);
                }
                Some(_) => {}
                None => {
                    hosts.insert(origin.host.clone(), addresses);
                }
            }
            origins.insert(origin);
        }
        Ok(EgressRules { origins, hosts })
    }
}

// Why a required profile refused a destination. Defined in meerkat-core so
// a coordinated refresh carries it to every reader of its shared result.
pub use meerkat_core::auth::HttpEgressRefusal;

/// Why a required profile cannot enforce its boundary. Nothing is sent, and
/// nothing falls back to a trusted-host client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[non_exhaustive]
pub enum HttpEgressUnavailable {
    #[error("the host egress policy is invalid")]
    InvalidPolicy,
    #[error("the host egress HTTP client could not be built")]
    ClientUnavailable,
    #[error("the credential owner does not enforce the host egress profile")]
    CredentialOwnerNotBound,
}

/// The typed local outcome of a required profile that did not admit an
/// effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum HttpEgressError {
    #[error(transparent)]
    Refused(HttpEgressRefusal),
    #[error(transparent)]
    Unavailable(HttpEgressUnavailable),
}

impl From<HttpEgressRefusal> for HttpEgressError {
    fn from(refusal: HttpEgressRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl From<HttpEgressUnavailable> for HttpEgressError {
    fn from(reason: HttpEgressUnavailable) -> Self {
        Self::Unavailable(reason)
    }
}

/// The refusal a failed request carries in its error source chain, when the
/// clients' own resolver refused the destination. Nothing was connected for
/// such a request.
pub fn egress_refusal_in(error: &(dyn std::error::Error + 'static)) -> Option<HttpEgressRefusal> {
    let mut current = Some(error);
    while let Some(error) = current {
        if let Some(refusal) = error.downcast_ref::<HttpEgressRefusal>() {
            return Some(*refusal);
        }
        current = error.source();
    }
    None
}

/// Immutable compiled destination rules, shared by admission and the
/// resolver.
#[derive(Debug)]
struct EgressRules {
    origins: BTreeSet<HttpEgressOrigin>,
    hosts: BTreeMap<EgressHost, HttpEgressAddresses>,
}

impl EgressRules {
    fn admit(&self, url: &Url) -> Result<(), HttpEgressRefusal> {
        let scheme = HttpEgressScheme::of(url).ok_or(HttpEgressRefusal::SchemeNotAdmitted)?;
        let host = EgressHost::of(url).ok_or(HttpEgressRefusal::InvalidDestination)?;
        let port = url
            .port_or_known_default()
            .ok_or(HttpEgressRefusal::InvalidDestination)?;
        let addresses = self
            .hosts
            .get(&host)
            .ok_or(HttpEgressRefusal::HostNotAdmitted)?;
        let origin = HttpEgressOrigin { scheme, host, port };
        if !self.origins.contains(&origin) {
            let scheme_admitted = self
                .origins
                .iter()
                .any(|admitted| admitted.host == origin.host && admitted.scheme == origin.scheme);
            return Err(if scheme_admitted {
                HttpEgressRefusal::PortNotAdmitted
            } else {
                HttpEgressRefusal::SchemeNotAdmitted
            });
        }
        // An IP-literal host is connected without any resolver.
        if let EgressHost::Ip(ip) = origin.host
            && !addresses.admits(ip)
        {
            return Err(HttpEgressRefusal::AddressNotAdmitted);
        }
        Ok(())
    }

    fn domain_addresses(&self, name: &str) -> Option<&HttpEgressAddresses> {
        self.hosts
            .get(&EgressHost::Domain(name.to_ascii_lowercase()))
    }

    /// Admit one complete resolution of an admitted host name. One address
    /// outside the host's set refuses the whole answer, so the connector
    /// never chooses among admitted and refused addresses.
    fn admit_resolved(&self, name: &str, resolved: &[SocketAddr]) -> Result<(), HttpEgressRefusal> {
        let addresses = self
            .domain_addresses(name)
            .ok_or(HttpEgressRefusal::HostNotAdmitted)?;
        if resolved
            .iter()
            .all(|address| addresses.admits(address.ip()))
        {
            Ok(())
        } else {
            Err(HttpEgressRefusal::AddressNotAdmitted)
        }
    }
}

/// How the egress resolver looks host names up: the system resolver the
/// connector would otherwise use or, in this crate's tests, a scripted
/// sequence of answers.
#[derive(Clone)]
enum EgressLookup {
    System,
    #[cfg(test)]
    Scripted(Arc<std::sync::Mutex<std::collections::VecDeque<Vec<IpAddr>>>>),
}

impl EgressLookup {
    async fn lookup(&self, name: &str) -> std::io::Result<Vec<SocketAddr>> {
        match self {
            Self::System => Ok(tokio::net::lookup_host((name, 0)).await?.collect()),
            #[cfg(test)]
            Self::Scripted(answers) => Ok(answers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .pop_front()
                .unwrap_or_default()
                .into_iter()
                .map(|ip| SocketAddr::new(ip, 0))
                .collect()),
        }
    }
}

/// The required clients' DNS resolver: the connector's own resolution, with
/// every resolved address admitted before anything is connected.
struct EgressResolver {
    rules: Arc<EgressRules>,
    lookup: EgressLookup,
}

type ResolveError = Box<dyn std::error::Error + Send + Sync>;

impl reqwest::dns::Resolve for EgressResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let rules = Arc::clone(&self.rules);
        let lookup = self.lookup.clone();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            // A host outside the policy is refused without a lookup.
            if rules.domain_addresses(&host).is_none() {
                return Err(ResolveError::from(HttpEgressRefusal::HostNotAdmitted));
            }
            let resolved = lookup.lookup(&host).await?;
            rules
                .admit_resolved(&host, &resolved)
                .map_err(ResolveError::from)?;
            Ok::<reqwest::dns::Addrs, ResolveError>(Box::new(resolved.into_iter()))
        })
    }
}

/// The required profile's own clients, built once at compile time.
struct CompiledHttpEgress {
    rules: Arc<EgressRules>,
    /// Ordinary MCP traffic: no redirect.
    ordinary: reqwest::Client,
    /// Protected tool calls: no redirect and no retry.
    protected: reqwest::Client,
    /// Credential endpoints (OAuth discovery, registration, exchange,
    /// refresh, userinfo): no redirect, each request bounded by
    /// [`crate::auth_oauth::CREDENTIAL_REQUEST_TIMEOUT`].
    credential: reqwest::Client,
}

impl CompiledHttpEgress {
    fn build(rules: EgressRules) -> Result<Self, HttpEgressUnavailable> {
        Self::build_with(rules, EgressLookup::System)
    }

    fn build_with(rules: EgressRules, lookup: EgressLookup) -> Result<Self, HttpEgressUnavailable> {
        let rules = Arc::new(rules);
        let resolver = Arc::new(EgressResolver {
            rules: Arc::clone(&rules),
            lookup,
        });
        // No required client follows a redirect: a redirected request's error
        // names the original request, not the hop, so a refused later hop
        // could not be told from an unsent request. Every 3xx answer is
        // returned to the caller, which refuses it by its status.
        let ordinary = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .dns_resolver(Arc::clone(&resolver))
            .build()
            .map_err(|_| HttpEgressUnavailable::ClientUnavailable)?;
        let protected = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .retry(reqwest::retry::never())
            .dns_resolver(Arc::clone(&resolver))
            .build()
            .map_err(|_| HttpEgressUnavailable::ClientUnavailable)?;
        // Credential requests are single exchanges: each is bounded like the
        // trusted-host credential client's. The ordinary and protected
        // clients carry MCP traffic and its streams, so they stay unbounded.
        let credential = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .dns_resolver(resolver)
            .connect_timeout(crate::auth_oauth::CREDENTIAL_CONNECT_TIMEOUT)
            .timeout(crate::auth_oauth::CREDENTIAL_REQUEST_TIMEOUT)
            .build()
            .map_err(|_| HttpEgressUnavailable::ClientUnavailable)?;
        Ok(Self {
            rules,
            ordinary,
            protected,
            credential,
        })
    }
}

enum EgressBinding {
    TrustedHost,
    Required(Result<CompiledHttpEgress, HttpEgressUnavailable>),
}

/// The host-selected HTTP egress boundary, retained by connections, routers
/// and the credential owner. Cloning shares the same compiled profile.
#[derive(Clone)]
pub struct HttpEgressProfile {
    binding: Arc<EgressBinding>,
}

impl std::fmt::Debug for HttpEgressProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.binding.as_ref() {
            EgressBinding::TrustedHost => f.write_str("HttpEgressProfile::TrustedHost"),
            EgressBinding::Required(Ok(compiled)) => f
                .debug_struct("HttpEgressProfile::Required")
                .field("origins", &compiled.rules.origins.len())
                .finish_non_exhaustive(),
            EgressBinding::Required(Err(reason)) => {
                write!(f, "HttpEgressProfile::Required(Unavailable({reason:?}))")
            }
        }
    }
}

impl Default for HttpEgressProfile {
    fn default() -> Self {
        Self::trusted_host()
    }
}

impl HttpEgressProfile {
    /// The existing host-managed behavior: no destination admission, the
    /// existing redirect protections and the ambient proxy configuration. An
    /// explicit absence of host egress enforcement.
    #[must_use]
    pub fn trusted_host() -> Self {
        Self {
            binding: Arc::new(EgressBinding::TrustedHost),
        }
    }

    /// Compile `policy` once into a required boundary. A compile failure is
    /// retained and refuses every use; nothing falls back to trusted-host
    /// clients.
    #[must_use]
    pub fn required(policy: HttpEgressPolicy) -> Self {
        let compiled = policy.compile().and_then(CompiledHttpEgress::build);
        Self {
            binding: Arc::new(EgressBinding::Required(compiled)),
        }
    }

    /// A required profile whose resolver answers from `answers`, one answer
    /// per lookup, instead of the system resolver.
    #[cfg(test)]
    fn required_with_answers(policy: HttpEgressPolicy, answers: Vec<Vec<IpAddr>>) -> Self {
        Self::required_with_lookup(
            policy,
            EgressLookup::Scripted(Arc::new(std::sync::Mutex::new(answers.into()))),
        )
    }

    #[cfg(test)]
    fn required_with_lookup(policy: HttpEgressPolicy, lookup: EgressLookup) -> Self {
        let compiled = policy
            .compile()
            .and_then(|rules| CompiledHttpEgress::build_with(rules, lookup));
        Self {
            binding: Arc::new(EgressBinding::Required(compiled)),
        }
    }

    /// Whether this profile enforces a host boundary.
    pub fn is_required(&self) -> bool {
        matches!(self.binding.as_ref(), EgressBinding::Required(_))
    }

    /// Whether `other` is this exact profile (the same compilation), as a
    /// credential owner bound to it reports.
    pub fn same_profile(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.binding, &other.binding)
    }

    fn compiled(&self) -> Option<Result<&CompiledHttpEgress, HttpEgressUnavailable>> {
        match self.binding.as_ref() {
            EgressBinding::TrustedHost => None,
            EgressBinding::Required(compiled) => Some(compiled.as_ref().map_err(|reason| *reason)),
        }
    }

    /// Admit one request URL. A trusted-host profile admits everything.
    pub fn admit_url(&self, url: &Url) -> Result<(), HttpEgressError> {
        match self.compiled() {
            None => Ok(()),
            Some(compiled) => Ok(compiled?.rules.admit(url)?),
        }
    }

    /// Admit one request URL as text, parsed exactly as the clients parse it.
    pub fn admit(&self, url: &str) -> Result<(), HttpEgressError> {
        match self.compiled() {
            None => Ok(()),
            Some(compiled) => {
                let compiled = compiled?;
                let url = Url::parse(url).map_err(|_| HttpEgressRefusal::InvalidDestination)?;
                Ok(compiled.rules.admit(&url)?)
            }
        }
    }

    /// The required ordinary client; `None` for a trusted-host profile,
    /// whose callers keep their existing clients.
    pub fn ordinary_client(&self) -> Option<Result<&reqwest::Client, HttpEgressUnavailable>> {
        self.compiled()
            .map(|compiled| compiled.map(|compiled| &compiled.ordinary))
    }

    /// The required protected-call client (no redirect, no retry); `None`
    /// for a trusted-host profile.
    pub fn protected_client(&self) -> Option<Result<&reqwest::Client, HttpEgressUnavailable>> {
        self.compiled()
            .map(|compiled| compiled.map(|compiled| &compiled.protected))
    }

    /// The required credential-endpoint client (no redirect, each request
    /// bounded like [`crate::auth_oauth::credential_http_client`]'s); `None`
    /// for a trusted-host profile.
    pub fn credential_client(&self) -> Option<Result<&reqwest::Client, HttpEgressUnavailable>> {
        self.compiled()
            .map(|compiled| compiled.map(|compiled| &compiled.credential))
    }
}

/// IPv4-mapped IPv6 addresses are compared as the IPv4 address they carry.
fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

/// Whether `ip` is a globally routable unicast address. Conservative: every
/// special-purpose range is refused.
pub fn is_public_unicast(ip: IpAddr) -> bool {
    match canonical_ip(ip) {
        IpAddr::V4(v4) => public_v4(v4),
        IpAddr::V6(v6) => public_v6(v6),
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    let special = a == 0 // "this network"
        || a == 10 // private
        || (a == 100 && (b & 0xc0) == 0x40) // shared address space (CGNAT)
        || a == 127 // loopback
        || (a == 169 && b == 254) // link-local
        || (a == 172 && (b & 0xf0) == 0x10) // private
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2) // documentation
        || (a == 192 && b == 88 && c == 99) // 6to4 relay anycast
        || (a == 192 && b == 168) // private
        || (a == 198 && (b & 0xfe) == 18) // benchmarking
        || (a == 198 && b == 51 && c == 100) // documentation
        || (a == 203 && b == 0 && c == 113) // documentation
        || a >= 224; // multicast, reserved, broadcast
    !special
}

fn public_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    // NAT64 well-known prefix: the embedded IPv4 address decides.
    if segments[..6] == [0x0064, 0xff9b, 0, 0, 0, 0] {
        let embedded = (u32::from(segments[6]) << 16) | u32::from(segments[7]);
        return public_v4(Ipv4Addr::from(embedded));
    }
    let special = (segments[0] & 0xff00) == 0 // ::/8 (unspecified, loopback, compatible, reserved)
        || (segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2] == 0x0001) // local-use NAT64
        || (segments[0] == 0x0100 && segments[1..4] == [0, 0, 0]) // discard-only
        || (segments[0] == 0x2001 && segments[1] < 0x0200) // IETF protocol assignments (incl. Teredo)
        || (segments[0] == 0x2001 && segments[1] == 0x0db8) // documentation
        || segments[0] == 0x2002 // 6to4
        || (segments[0] == 0x3fff && segments[1] < 0x1000) // documentation
        || (segments[0] & 0xfe00) == 0xfc00 // unique local
        || (segments[0] & 0xffc0) == 0xfe80 // link-local
        || (segments[0] & 0xffc0) == 0xfec0 // site-local (deprecated)
        || (segments[0] & 0xff00) == 0xff00; // multicast
    !special
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn origin(text: &str) -> HttpEgressOrigin {
        HttpEgressOrigin::parse(text).unwrap()
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn refused(profile: &HttpEgressProfile, url: &str) -> Option<HttpEgressRefusal> {
        match profile.admit(url) {
            Ok(()) => None,
            Err(HttpEgressError::Refused(refusal)) => Some(refusal),
            Err(other) => panic!("expected a refusal for {url}, got {other:?}"),
        }
    }

    #[test]
    fn origins_are_exact_and_normalized_like_request_urls() {
        assert_eq!(
            origin("HTTPS://API.Example.com"),
            origin("https://api.example.com:443/")
        );
        assert_eq!(origin("http://127.1:80"), origin("http://127.0.0.1"));
        assert_eq!(origin("http://[::1]:8080").port(), 8080);
        for invalid in [
            "https://user:secret@api.example.com",
            "https://api.example.com/path",
            "https://api.example.com/?q=1",
            "https://api.example.com/#f",
            "ftp://api.example.com",
            "not a url",
        ] {
            assert_eq!(
                HttpEgressOrigin::parse(invalid),
                Err(HttpEgressOriginError),
                "{invalid}"
            );
        }
    }

    #[test]
    fn a_trusted_host_profile_admits_everything_and_keeps_existing_clients() {
        let profile = HttpEgressProfile::trusted_host();
        assert!(!profile.is_required());
        assert_eq!(profile.admit("http://169.254.169.254/latest"), Ok(()));
        assert!(profile.ordinary_client().is_none());
        assert!(profile.protected_client().is_none());
        assert!(profile.credential_client().is_none());
    }

    #[test]
    fn admission_is_exact_by_scheme_host_and_effective_port() {
        let profile = HttpEgressProfile::required(
            HttpEgressPolicy::new()
                .allow_public(origin("https://api.example.com"))
                .allow_local(origin("http://127.0.0.1:8931"), [ip("127.0.0.1")]),
        );
        assert!(profile.is_required());
        assert_eq!(refused(&profile, "https://api.example.com/mcp?x=1"), None);
        assert_eq!(refused(&profile, "https://API.example.com:443/mcp"), None);
        assert_eq!(refused(&profile, "http://127.0.0.1:8931/mcp"), None);
        assert_eq!(
            refused(&profile, "https://other.example.com/mcp"),
            Some(HttpEgressRefusal::HostNotAdmitted)
        );
        assert_eq!(
            refused(&profile, "https://api.example.com:8443/mcp"),
            Some(HttpEgressRefusal::PortNotAdmitted)
        );
        assert_eq!(
            refused(&profile, "http://api.example.com/mcp"),
            Some(HttpEgressRefusal::SchemeNotAdmitted)
        );
        assert_eq!(
            refused(&profile, "https://127.0.0.1:8931/mcp"),
            Some(HttpEgressRefusal::SchemeNotAdmitted)
        );
        assert_eq!(
            refused(&profile, "http://localhost:8931/mcp"),
            Some(HttpEgressRefusal::HostNotAdmitted)
        );
        assert_eq!(
            refused(&profile, "ftp://api.example.com/"),
            Some(HttpEgressRefusal::SchemeNotAdmitted)
        );
        assert_eq!(
            refused(&profile, "::not-a-url"),
            Some(HttpEgressRefusal::InvalidDestination)
        );
    }

    #[test]
    fn a_literal_address_needs_deliberate_authority_and_loopback_is_never_implicit() {
        let public_only = HttpEgressProfile::required(
            HttpEgressPolicy::new()
                .allow_public(origin("http://127.0.0.1:8931"))
                .allow_public(origin("http://[::1]:8931"))
                .allow_public(origin("http://10.1.2.3")),
        );
        for url in [
            "http://127.0.0.1:8931/",
            "http://[::1]:8931/",
            "http://10.1.2.3/",
        ] {
            assert_eq!(
                refused(&public_only, url),
                Some(HttpEgressRefusal::AddressNotAdmitted),
                "{url}"
            );
        }
        let deliberate = HttpEgressProfile::required(
            HttpEgressPolicy::new().allow_local(origin("http://127.0.0.1:8931"), [ip("127.0.0.1")]),
        );
        assert_eq!(refused(&deliberate, "http://127.0.0.1:8931/"), None);
    }

    #[test]
    fn conflicting_or_empty_address_sets_are_unavailable_never_trusted() {
        let conflicting = HttpEgressProfile::required(
            HttpEgressPolicy::new()
                .allow_public(origin("https://svc.example.com"))
                .allow_local(origin("http://svc.example.com:8080"), [ip("10.0.0.5")]),
        );
        let empty = HttpEgressProfile::required(
            HttpEgressPolicy::new().allow_local(origin("http://svc.local:80"), []),
        );
        for profile in [conflicting, empty] {
            assert!(profile.is_required());
            assert_eq!(
                profile.admit("https://svc.example.com/"),
                Err(HttpEgressError::Unavailable(
                    HttpEgressUnavailable::InvalidPolicy
                ))
            );
            assert!(matches!(
                profile.ordinary_client(),
                Some(Err(HttpEgressUnavailable::InvalidPolicy))
            ));
            assert!(matches!(
                profile.credential_client(),
                Some(Err(HttpEgressUnavailable::InvalidPolicy))
            ));
        }
    }

    #[test]
    fn public_classification_refuses_every_special_purpose_range() {
        for address in [
            "0.0.0.0",
            "10.0.0.1",
            "100.64.0.1",
            "100.127.255.254",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "172.31.255.255",
            "192.0.0.9",
            "192.0.2.1",
            "192.88.99.1",
            "192.168.1.1",
            "198.18.0.1",
            "198.19.255.255",
            "198.51.100.7",
            "203.0.113.9",
            "224.0.0.1",
            "240.0.0.1",
            "255.255.255.255",
            "::",
            "::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::127.0.0.1",
            "64:ff9b::7f00:1",
            "64:ff9b:1::1",
            "100::1",
            "2001::1",
            "2001:db8::1",
            "2002::1",
            "3fff::1",
            "fc00::1",
            "fd12::1",
            "fe80::1",
            "fec0::1",
            "ff02::1",
        ] {
            assert!(
                !is_public_unicast(ip(address)),
                "{address} must not be public"
            );
        }
        for address in [
            "1.1.1.1",
            "8.8.8.8",
            "100.63.255.255",
            "100.128.0.1",
            "172.15.255.255",
            "172.32.0.1",
            "192.169.0.1",
            "198.17.255.255",
            "198.20.0.1",
            "223.255.255.254",
            "::ffff:8.8.8.8",
            "64:ff9b::808:808",
            "2001:4860:4860::8888",
            "2606:4700::1111",
            "2a00:1450::1",
        ] {
            assert!(is_public_unicast(ip(address)), "{address} must be public");
        }
    }

    async fn counting_listener() -> (u16, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let arrivals = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&arrivals);
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buffer = [0_u8; 4096];
                    let _ = stream.read(&mut buffer).await;
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                        )
                        .await;
                });
            }
        });
        (port, arrivals)
    }

    /// The clients' own resolver refuses a host name whose resolution is
    /// outside its address set; the refusal travels typed in the error chain
    /// and nothing is connected.
    #[tokio::test]
    async fn the_connector_resolver_refuses_loopback_without_deliberate_authority() {
        let (port, arrivals) = counting_listener().await;
        let profile = HttpEgressProfile::required(
            HttpEgressPolicy::new().allow_public(origin(&format!("http://localhost:{port}"))),
        );
        let url = format!("http://localhost:{port}/");
        assert_eq!(profile.admit(&url), Ok(()), "the URI itself is admitted");
        for client in [
            profile.ordinary_client(),
            profile.protected_client(),
            profile.credential_client(),
        ] {
            let error = client
                .unwrap()
                .unwrap()
                .get(&url)
                .send()
                .await
                .expect_err("loopback resolution is outside a public-only set");
            assert_eq!(
                egress_refusal_in(&error),
                Some(HttpEgressRefusal::AddressNotAdmitted),
                "{error:?}"
            );
            assert_eq!(error.url().map(Url::as_str), Some(url.as_str()));
        }
        assert_eq!(arrivals.load(Ordering::SeqCst), 0, "nothing was connected");

        let deliberate = HttpEgressProfile::required(HttpEgressPolicy::new().allow_local(
            origin(&format!("http://localhost:{port}")),
            [ip("127.0.0.1"), ip("::1")],
        ));
        let response = deliberate
            .credential_client()
            .unwrap()
            .unwrap()
            .get(&url)
            .send()
            .await
            .expect("an exactly authorized local service is reachable");
        assert!(response.status().is_success());
        assert_eq!(arrivals.load(Ordering::SeqCst), 1);
    }

    /// Only a required profile's credential client is bounded: its ordinary
    /// and protected clients carry MCP traffic and streams, so a slow answer
    /// still reaches them. Virtual time.
    #[tokio::test(start_paused = true)]
    async fn only_the_credential_client_of_a_required_profile_is_bounded() {
        let never = crate::auth_oauth::redirect_fixture::spawn_slow_endpoint(None).await;
        let slow = crate::auth_oauth::redirect_fixture::spawn_slow_endpoint(Some(
            crate::auth_oauth::CREDENTIAL_REQUEST_TIMEOUT * 4,
        ))
        .await;
        let local = |port: u16| {
            HttpEgressPolicy::new().allow_local(
                origin(&format!("http://localhost:{port}")),
                [ip("127.0.0.1"), ip("::1")],
            )
        };
        let bounded = HttpEgressProfile::required(local(never.port()));
        let request = bounded
            .credential_client()
            .unwrap()
            .unwrap()
            .post(format!("http://localhost:{}/token", never.port()))
            .send();
        let error =
            tokio::time::timeout(crate::auth_oauth::CREDENTIAL_REQUEST_TIMEOUT * 10, request)
                .await
                .expect("the credential client ends the request itself")
                .expect_err("a credential request its endpoint never answers ends");
        assert!(error.is_timeout(), "{error:?}");

        let unbounded = HttpEgressProfile::required(local(slow.port()));
        for client in [unbounded.ordinary_client(), unbounded.protected_client()] {
            let response = client
                .unwrap()
                .unwrap()
                .post(format!("http://localhost:{}/mcp", slow.port()))
                .send()
                .await
                .expect("a slow MCP answer still arrives");
            assert!(response.status().is_success());
        }
    }

    #[tokio::test]
    async fn a_host_outside_the_policy_is_refused_by_the_resolver_without_a_lookup() {
        let profile = HttpEgressProfile::required(
            HttpEgressPolicy::new().allow_public(origin("https://api.example.com")),
        );
        let error = profile
            .ordinary_client()
            .unwrap()
            .unwrap()
            .get("http://unlisted.invalid/")
            .send()
            .await
            .expect_err("an unlisted host never resolves");
        assert_eq!(
            egress_refusal_in(&error),
            Some(HttpEgressRefusal::HostNotAdmitted)
        );
    }

    /// An answer that mixes admitted and refused addresses is refused whole,
    /// before anything is connected.
    #[tokio::test]
    async fn a_mixed_resolution_is_refused_whole() {
        let rules = HttpEgressPolicy::new()
            .allow_local(origin("http://svc.test:8931"), [ip("127.0.0.1")])
            .allow_public(origin("https://api.example.com"))
            .compile()
            .unwrap();
        let at = |text: &str| SocketAddr::new(ip(text), 0);
        assert_eq!(rules.admit_resolved("svc.test", &[at("127.0.0.1")]), Ok(()));
        assert_eq!(
            rules.admit_resolved("svc.test", &[at("127.0.0.1"), at("10.9.9.9")]),
            Err(HttpEgressRefusal::AddressNotAdmitted)
        );
        assert_eq!(
            rules.admit_resolved("api.example.com", &[at("8.8.8.8"), at("10.0.0.1")]),
            Err(HttpEgressRefusal::AddressNotAdmitted)
        );
        assert_eq!(
            rules.admit_resolved("api.example.com", &[at("8.8.8.8"), at("2606:4700::1111")]),
            Ok(())
        );
        assert_eq!(
            rules.admit_resolved("other.test", &[at("8.8.8.8")]),
            Err(HttpEgressRefusal::HostNotAdmitted)
        );

        let (port, arrivals) = counting_listener().await;
        let profile = HttpEgressProfile::required_with_answers(
            HttpEgressPolicy::new().allow_local(
                origin(&format!("http://svc.test:{port}")),
                [ip("127.0.0.1")],
            ),
            vec![vec![ip("127.0.0.1"), ip("10.9.9.9")]],
        );
        let error = profile
            .ordinary_client()
            .unwrap()
            .unwrap()
            .get(format!("http://svc.test:{port}/"))
            .send()
            .await
            .expect_err("a mixed answer is refused");
        assert_eq!(
            egress_refusal_in(&error),
            Some(HttpEgressRefusal::AddressNotAdmitted)
        );
        assert_eq!(arrivals.load(Ordering::SeqCst), 0);
    }

    /// No required client follows a redirect, not even within the origin: a
    /// redirected request's error would name the original request rather
    /// than the refused hop, so a later hop could not be told from an unsent
    /// request. The 3xx answer is returned to the caller unfollowed, and no
    /// second resolution or connection happens.
    #[tokio::test]
    async fn no_required_client_follows_a_redirect_hop() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let arrivals = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&arrivals);
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            while let Ok((mut stream, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer).await;
                let answer = format!(
                    "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://svc.test:{port}/next\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(answer.as_bytes()).await;
            }
        });
        let answers = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from(
            vec![
                vec![ip("127.0.0.1")],
                vec![ip("127.0.0.1")],
                vec![ip("127.0.0.1")],
                vec![ip("10.9.9.9")],
            ],
        )));
        let profile = HttpEgressProfile::required_with_lookup(
            HttpEgressPolicy::new().allow_local(
                origin(&format!("http://svc.test:{port}")),
                [ip("127.0.0.1")],
            ),
            EgressLookup::Scripted(Arc::clone(&answers)),
        );
        let requested = format!("http://svc.test:{port}/start");
        for client in [
            profile.ordinary_client(),
            profile.protected_client(),
            profile.credential_client(),
        ] {
            let response = client
                .unwrap()
                .unwrap()
                .post(&requested)
                .body("{}")
                .send()
                .await
                .expect("the redirect answer itself is returned");
            assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
            assert_eq!(response.url().as_str(), requested);
        }
        assert_eq!(
            arrivals.load(Ordering::SeqCst),
            3,
            "one first hop per client"
        );
        assert_eq!(
            answers.lock().unwrap().len(),
            1,
            "no hop was resolved again"
        );
    }
}
