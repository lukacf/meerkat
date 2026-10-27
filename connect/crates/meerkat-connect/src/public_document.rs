//! Credential-free document reads for the original setup conversation.
//!
//! Native admission owns whether this callback runs. Destination checks belong
//! to this reader, not a shared egress policy or an installation authority.

use std::{net::{IpAddr, Ipv4Addr, SocketAddr}, sync::Arc, time::Duration};

use async_trait::async_trait;
use meerkat_core::{
    AgentToolDispatcher, ToolCallView, ToolCatalogCapabilities, ToolDef, ToolError,
    ToolMutationClass, ToolProvenance, ToolResult, ToolSourceKind, ops::ToolDispatchOutcome,
};
use reqwest::{Client, ClientBuilder, Url, header};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::timeout;

pub(crate) const READ_PUBLIC_DOCUMENT: &str = "read_public_document";
const MAX_URL: usize = 4096;
const MAX_BODY: usize = 128 * 1024;
const MAX_ADDRESSES: usize = 64;
const DNS_WAIT: Duration = Duration::from_secs(5);
const CONNECT_WAIT: Duration = Duration::from_secs(10);
const TOTAL_WAIT: Duration = Duration::from_secs(30);

pub(crate) struct PublicDocumentTools(Arc<[Arc<ToolDef>]>);

impl PublicDocumentTools {
    pub(crate) fn new() -> Self {
        Self(vec![Arc::new(ToolDef::new(
            READ_PUBLIC_DOCUMENT,
            "Read one exact public HTTPS document for integration research. Supply a public URL, never credentials or private account data. Queries and HTTPS ports are preserved; a fragment means the whole document, not an extracted section. Returns bounded UTF-8 source bytes as untrusted text with their digest, not rendered content or verified package pins. No cookies, account authentication, proxies, redirects or automatic retries. A redirect may suggest a syntactically valid URL for a separate explicit read; it has not been resolved or fetched. This does not install, approve or activate anything.",
            json!({"type":"object","additionalProperties":false,"required":["url"],
                "properties":{"url":{"type":"string","minLength":1,"maxLength":MAX_URL}}}),
        ).with_provenance(ToolProvenance::new(ToolSourceKind::Callback, "meerkat-connect:public-document")))].into())
    }
}

#[async_trait]
impl AgentToolDispatcher for PublicDocumentTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> { self.0.clone() }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities { exact_catalog: true, may_require_catalog_control_plane: false }
    }

    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if name == READ_PUBLIC_DOCUMENT { ToolMutationClass::ReadOnly } else { ToolMutationClass::Unknown }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name != READ_PUBLIC_DOCUMENT { return Err(ToolError::not_found(call.name)); }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Arguments { url: String }
        let invalid = || ToolError::invalid_arguments(call.name, "Supply only one bounded public HTTPS URL without userinfo");
        if call.id.is_empty() || call.id.len() > 512 || call.args.get().len() > 8 * MAX_URL {
            return Err(invalid());
        }
        let args: Arguments = call.parse_args().map_err(|_| invalid())?;
        let result = match timeout(TOTAL_WAIT, read_document(&args.url)).await {
            Ok(result) => result,
            Err(_) => Err(Refusal::Timeout),
        };
        let (value, is_error) = match result {
            Ok(value) => (value, false),
            Err(refusal) => (refusal.value(), true),
        };
        Ok(ToolResult::new(call.id.into(), value.to_string(), is_error).into())
    }
}

#[derive(Debug)]
enum Refusal {
    Url,
    Destination,
    Dns,
    Timeout,
    Transport,
    Redirect(Option<String>),
    Status(u16),
    MediaType,
    Encoding,
    TooLarge,
    Utf8,
}

impl Refusal {
    fn value(&self) -> Value {
        let (code, message) = match self {
            Self::Url => ("invalid_url", "Use a bounded HTTPS URL without userinfo, control characters or a zero port"),
            Self::Destination => ("destination_refused", "The destination must resolve exclusively to public unicast addresses"),
            Self::Dns => ("dns_unavailable", "The destination could not be resolved within the reader's address bound"),
            Self::Timeout => ("read_timeout", "The document read did not finish within its time bound; no retry was started"),
            Self::Transport => ("transport_failed", "The HTTPS read failed; transport details are withheld and no retry was started"),
            Self::Redirect(_) => ("redirect_refused", "No redirect was followed. Any suggested URL is unverified and requires a new explicit read"),
            Self::Status(_) => ("http_status", "The server did not return a complete successful document; no body was accepted"),
            Self::MediaType => ("unsupported_media_type", "Only declared text, JSON or XHTML documents are supported"),
            Self::Encoding => ("unsupported_encoding", "Compressed or non-UTF-8 document encodings are unsupported"),
            Self::TooLarge => ("document_too_large", "The document exceeds the reader's byte bound; no partial source was returned"),
            Self::Utf8 => ("invalid_utf8", "The response is not a UTF-8 source document"),
        };
        let mut value = json!({"error":code,"message":message});
        if let Self::Status(status) = self { value["status"] = json!(status); }
        if let Self::Redirect(Some(location)) = self { value["suggested_url"] = json!(location); }
        value
    }
}

struct DocumentUrl { requested: String, fetch: Url, host: String }

impl DocumentUrl {
    fn parse(value: &str) -> Result<Self, Refusal> {
        if value.is_empty() || value.len() > MAX_URL
            || value.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\')
        { return Err(Refusal::Url); }
        let mut fetch = Url::parse(value).map_err(|_| Refusal::Url)?;
        let authority = value.split_once("://").map(|(_, tail)| tail.split(['/', '?', '#']).next().unwrap_or(""));
        if fetch.scheme() != "https" || !fetch.username().is_empty() || fetch.password().is_some()
            || authority.is_none() || authority.is_some_and(|authority| authority.contains('@')) || fetch.port() == Some(0)
        { return Err(Refusal::Url); }
        let host = fetch.host_str().ok_or(Refusal::Url)?.to_owned();
        if host.len() > 253 { return Err(Refusal::Url); }
        if let Some(ip) = literal_address(&host) && !public_address(ip) {
            return Err(Refusal::Destination);
        }
        fetch.set_fragment(None);
        Ok(Self { requested: value.into(), fetch, host })
    }

    async fn resolve(self) -> Result<Target, Refusal> {
        let port = self.fetch.port_or_known_default().ok_or(Refusal::Url)?;
        let addresses = if let Some(ip) = literal_address(&self.host) {
            vec![SocketAddr::new(ip, port)]
        } else {
            // Dropping lookup_host stops this read from progressing to HTTP.
            // An entered platform resolver job itself is not revocable.
            timeout(DNS_WAIT, tokio::net::lookup_host((self.host.as_str(), port)))
                .await.map_err(|_| Refusal::Timeout)?
                .map_err(|_| Refusal::Dns)?
                .take(MAX_ADDRESSES + 1).collect()
        };
        admit_addresses(&addresses)?;
        Ok(Target { document: self, addresses })
    }
}

fn literal_address(host: &str) -> Option<IpAddr> {
    host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host).parse().ok()
}

fn admit_addresses(addresses: &[SocketAddr]) -> Result<(), Refusal> {
    if addresses.is_empty() || addresses.len() > MAX_ADDRESSES { return Err(Refusal::Dns); }
    if addresses.iter().any(|addr| !public_address(addr.ip())) { return Err(Refusal::Destination); }
    Ok(())
}

// Public unicast only. Special-use source: IANA IPv4/IPv6 special-purpose
// registries, checked 2026-10-07 (registry update 2025-10-09).
// https://www.iana.org/assignments/iana-ipv4-special-registry/
// https://www.iana.org/assignments/iana-ipv6-special-registry/
fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => public_v4(ip),
        IpAddr::V6(ip) => {
            // Mapped destinations use the embedded IPv4 routing identity.
            if let Some(v4) = ip.to_ipv4_mapped() { return public_v4(v4); }
            let bits = u128::from(ip);
            // Current ordinary IPv6 global unicast allocation. Translation,
            // multicast, local, reserved and legacy compatible forms are not
            // alternative routes around the destination check.
            if bits >> 125 != 1 { return false; }
            if bits >> 96 == 0x20010db8 || bits >> 108 == 0x3fff0
                || bits >> 112 == 0x2002 { return false; }
            if bits >> 105 == (0x2001u128 << 7) {
                return matches!(bits, 0x20010001000000000000000000000001
                    | 0x20010001000000000000000000000002
                    | 0x20010001000000000000000000000003)
                    || bits >> 96 == 0x20010003
                    || bits >> 80 == 0x200100040112
                    || bits >> 100 == 0x2001002
                    || bits >> 100 == 0x2001003;
            }
            true
        }
    }
}

fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, d] = ip.octets();
    if matches!((a, b, c, d), (192, 0, 0, 9 | 10)) { return true; }
    !(matches!(a, 0 | 10 | 127 | 224..=255)
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && ((b == 0 && matches!(c, 0 | 2)) || (b == 88 && c == 99) || b == 168))
        || (a == 198 && (matches!(b, 18 | 19) || (b == 51 && c == 100)))
        || (a == 203 && b == 0 && c == 113))
}

struct Target { document: DocumentUrl, addresses: Vec<SocketAddr> }

fn client(target: &Target) -> ClientBuilder {
    Client::builder().https_only(true)
        .resolve_to_addrs(&target.document.host, &target.addresses)
        .redirect(reqwest::redirect::Policy::none()).retry(reqwest::retry::never())
        .no_proxy().referer(false).no_gzip().no_brotli().no_zstd().no_deflate()
        .connect_timeout(CONNECT_WAIT).timeout(TOTAL_WAIT)
}

async fn read_document(url: &str) -> Result<Value, Refusal> {
    let target = DocumentUrl::parse(url)?.resolve().await?;
    let client = client(&target).build().map_err(|_| Refusal::Transport)?;
    fetch_document(&target.document, client).await
}

async fn fetch_document(document: &DocumentUrl, client: Client) -> Result<Value, Refusal> {
    let mut response = client.get(document.fetch.clone())
        .header(header::ACCEPT, "text/*, application/json, application/xhtml+xml")
        .header(header::ACCEPT_ENCODING, "identity")
        .send().await.map_err(transport_error)?;
    if response.status().is_redirection() {
        let suggested = response.headers().get(header::LOCATION)
            .and_then(|value| value.to_str().ok())
            .filter(|value| value.len() <= MAX_URL
                && !value.chars().any(|c| c.is_control() || c.is_whitespace() || c == '\\'))
            .and_then(|value| document.fetch.join(value).ok())
            .and_then(|url| DocumentUrl::parse(url.as_str()).ok())
            .map(|document| document.requested);
        return Err(Refusal::Redirect(suggested));
    }
    if response.status() != reqwest::StatusCode::OK { return Err(Refusal::Status(response.status().as_u16())); }
    if response.headers().get_all(header::CONTENT_ENCODING).iter().any(|value| !value.to_str().is_ok_and(|value| value.trim().eq_ignore_ascii_case("identity"))) {
        return Err(Refusal::Encoding);
    }
    let media_type = document_media_type(response.headers())?;
    if response.content_length().is_some_and(|size| size > MAX_BODY as u64) { return Err(Refusal::TooLarge); }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if bytes.len() + chunk.len() > MAX_BODY { return Err(Refusal::TooLarge); }
        bytes.extend_from_slice(&chunk);
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| Refusal::Utf8)?;
    Ok(json!({"requested_url":document.requested,"document_url":document.fetch.as_str(),
        "media_type":media_type,"byte_count":bytes.len(),
        "sha256":format!("{:x}", Sha256::digest(&bytes)),"text":text}))
}

fn transport_error(error: reqwest::Error) -> Refusal {
    if error.is_timeout() { Refusal::Timeout } else { Refusal::Transport }
}

fn document_media_type(headers: &header::HeaderMap) -> Result<String, Refusal> {
    let mut values = headers.get_all(header::CONTENT_TYPE).iter();
    let value = values.next().ok_or(Refusal::MediaType)?.to_str().map_err(|_| Refusal::MediaType)?;
    if values.next().is_some() { return Err(Refusal::MediaType); }
    let mut parts = value.split(';');
    let kind = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    if !(kind.starts_with("text/") && kind.len() > 5
        || kind == "application/json" || kind == "application/xhtml+xml"
        || kind.starts_with("application/") && kind.ends_with("+json")) {
        return Err(Refusal::MediaType);
    }
    for parameter in parts {
        if let Some((name, value)) = parameter.trim().split_once('=')
            && name.trim().eq_ignore_ascii_case("charset")
            && !matches!(value.trim().trim_matches('"').to_ascii_lowercase().as_str(), "utf-8" | "utf8" | "us-ascii")
        { return Err(Refusal::Encoding); }
    }
    Ok(kind)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, atomic::{AtomicUsize, Ordering}};
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener, sync::{Notify, oneshot}};
    use tokio_rustls::{TlsAcceptor, rustls::{self, pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer}}};

    // Public test material only, valid for document-reader.invalid. Neither
    // its CA nor its loopback destination is admitted by production callers.
    const CA: &[u8] = include_bytes!("../tests/fixtures/public_document/test-ca.der");
    const CERT: &[u8] = include_bytes!("../tests/fixtures/public_document/test-server.der");
    const KEY: &[u8] = include_bytes!("../tests/fixtures/public_document/test-server-key.der");
    const BOUND: Duration = Duration::from_secs(5);

    #[test]
    fn url_and_address_admission_preserve_public_document_choices() {
        let url = DocumentUrl::parse("https://docs.example:8443/guide?version=2&lang=en#install").unwrap();
        assert_eq!(url.fetch.as_str(), "https://docs.example:8443/guide?version=2&lang=en");
        assert!(url.requested.ends_with("#install"));
        for url in ["http://example.com/", "file:///etc/passwd", "https:example.com",
            "https://user@example.com", "https://@example.com", "https://user:password@example.com",
            "https://example.com:0/", "https://example.com/\nsecret", "https://example.com\\other"] {
            assert!(DocumentUrl::parse(url).is_err(), "{url}");
        }
        for url in ["https://127.1/", "https://2130706433/", "https://0x7f000001/",
            "https://[::1]/", "https://[::ffff:127.0.0.1]/"] {
            assert!(matches!(DocumentUrl::parse(url), Err(Refusal::Destination)), "{url}");
        }
        for address in ["0.0.0.1", "10.0.0.1", "100.64.0.1", "100.127.255.255",
            "127.0.0.1", "169.254.169.254", "172.16.0.1", "172.31.255.255",
            "192.0.0.8", "192.0.0.170", "192.0.2.1", "192.88.99.2", "192.168.1.1",
            "198.18.0.1", "198.19.255.255", "198.51.100.1", "203.0.113.1",
            "224.0.0.1", "240.0.0.1", "255.255.255.255", "::", "::1", "::127.0.0.1",
            "::ffff:10.0.0.1", "64:ff9b::7f00:1", "64:ff9b:1::1", "100::1",
            "100:0:0:1::1", "2001::1", "2001:2::1", "2001:10::1", "2001:db8::1",
            "2002:7f00:1::", "3fff::1", "3fff:fff::1", "5f00::1", "fc00::1", "fe80::1", "ff02::1"] {
            assert!(!public_address(address.parse().unwrap()), "{address}");
        }
        for address in ["1.1.1.1", "8.8.8.8", "100.63.255.255", "100.128.0.0",
            "172.15.255.255", "172.32.0.0", "192.0.0.9", "192.0.0.10",
            "::ffff:8.8.8.8", "2606:4700:4700::1111", "2001:4860:4860::8888",
            "2001:1::1", "2001:1::2", "2001:1::3", "2001:3::1", "2001:4:112::1",
            "2001:20::1", "2001:30::1", "3ffe:ffff::1"] {
            assert!(public_address(address.parse().unwrap()), "{address}");
        }
        assert!(admit_addresses(&[]).is_err());
        assert!(admit_addresses(&["8.8.8.8:443".parse().unwrap(), "127.0.0.1:443".parse().unwrap()]).is_err());
        assert!(admit_addresses(&["8.8.8.8:443".parse().unwrap(); MAX_ADDRESSES + 1]).is_err());
        assert!(admit_addresses(&["8.8.8.8:8443".parse().unwrap(), "[2606:4700::1111]:8443".parse().unwrap()]).is_ok());
    }

    #[tokio::test]
    async fn public_constructor_refuses_actual_local_dns_and_closed_arguments() {
        assert!(matches!(DocumentUrl::parse("https://localhost/").unwrap().resolve().await, Err(Refusal::Destination)));
        let tools = PublicDocumentTools::new();
        assert_eq!(tools.tool_mutation_class(READ_PUBLIC_DOCUMENT), ToolMutationClass::ReadOnly);
        assert!(tools.tools()[0].provenance.is_some());
        for args in [r#"{"url":"https://127.0.0.1/","account":"other"}"#,
            r#"{"url":"https://127.0.0.1/","url":"https://8.8.8.8/"}"#, r#"{}"#] {
            let raw = serde_json::value::RawValue::from_string(args.into()).unwrap();
            assert!(tools.dispatch(ToolCallView { id: "invalid", name: READ_PUBLIC_DOCUMENT, args: &raw }).await.is_err());
        }
    }

    struct NoDns(Arc<AtomicUsize>);
    impl reqwest::dns::Resolve for NoDns {
        fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Err(std::io::Error::other("unexpected test DNS lookup").into()) })
        }
    }

    struct Server {
        address: SocketAddr,
        requests: Arc<Mutex<Vec<(String, String)>>>,
        seen: Arc<Notify>,
        closed: Arc<Notify>,
        stop: Option<oneshot::Sender<()>>,
        task: Option<tokio::task::JoinHandle<()>>,
    }

    impl Server {
        async fn start(reply: Vec<u8>, hold: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions().unwrap().with_no_client_auth()
                .with_single_cert(vec![CertificateDer::from(CERT.to_vec())],
                    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec()))).unwrap();
            let acceptor = TlsAcceptor::from(Arc::new(config));
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = requests.clone();
            let seen = Arc::new(Notify::new());
            let observed = seen.clone();
            let closed = Arc::new(Notify::new());
            let settled = closed.clone();
            let (stop, mut stopped) = oneshot::channel();
            let task = tokio::spawn(async move {
                loop {
                    let stream = tokio::select! {
                        biased;
                        _ = &mut stopped => break,
                        result = listener.accept() => result.unwrap().0,
                    };
                    let Ok(Ok(mut stream)) = timeout(BOUND, acceptor.accept(stream)).await else { continue; };
                    let mut bytes = Vec::new();
                    while !bytes.ends_with(b"\r\n\r\n") {
                        let mut chunk = [0u8; 1024];
                        let n = timeout(BOUND, stream.read(&mut chunk)).await.unwrap().unwrap();
                        if n == 0 { break; }
                        bytes.extend_from_slice(&chunk[..n]);
                        assert!(bytes.len() <= 16 * 1024);
                    }
                    if bytes.is_empty() { continue; }
                    captured.lock().unwrap().push((
                        String::from_utf8(bytes).unwrap(), stream.get_ref().1.server_name().unwrap().to_owned(),
                    ));
                    // Refusal may close the socket before a larger body has
                    // finished writing. The client result remains the oracle.
                    let written = timeout(BOUND, stream.write_all(&reply)).await.unwrap();
                    if written.is_err() { continue; }
                    let _ = timeout(BOUND, stream.flush()).await.unwrap();
                    observed.notify_one();
                    if hold {
                        let mut byte = [0u8; 1];
                        let eof = timeout(BOUND, stream.read(&mut byte)).await.unwrap();
                        assert!(matches!(eof, Ok(0)) || eof.is_err(), "no second request after cancellation");
                        settled.notify_one();
                    }
                    let _ = timeout(BOUND, stream.shutdown()).await;
                }
            });
            Self { address, requests, seen, closed, stop: Some(stop), task: Some(task) }
        }

        fn target(&self, hostname: &str) -> Target {
            // This private test construction skips public address admission;
            // it tests the same connector, not a public-constructor success.
            Target { document: DocumentUrl::parse(&format!("https://{hostname}:{}/guide?lang=en#section", self.address.port())).unwrap(), addresses: vec![self.address] }
        }

        fn client(&self, target: &Target, trust: bool, dns: Arc<AtomicUsize>) -> ClientBuilder {
            let builder = super::client(target).dns_resolver(Arc::new(NoDns(dns)));
            if trust { builder.add_root_certificate(reqwest::Certificate::from_der(CA).unwrap()) } else { builder }
        }

        async fn finish(mut self) {
            let _ = self.stop.take().unwrap().send(());
            let task = self.task.as_mut().unwrap();
            match timeout(BOUND, &mut *task).await {
                Ok(result) => result.unwrap(),
                Err(_) => {
                    task.abort();
                    let _ = timeout(BOUND, &mut *task).await;
                    panic!("document fixture required forced cleanup");
                }
            }
            let _ = self.task.take();
        }
    }

    impl Drop for Server {
        fn drop(&mut self) { if let Some(task) = self.task.take() { task.abort(); } }
    }

    fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut reply = format!("HTTP/1.1 {status}\r\nConnection: close\r\n{headers}\r\n").into_bytes();
        reply.extend_from_slice(body);
        reply
    }

    #[tokio::test]
    async fn pinned_socket_preserves_tls_host_query_and_exact_body_without_credentials() {
        let body = "# Public guide\nChoose the community edition.\n";
        let server = Server::start(response("200 OK", &format!("Content-Type: text/markdown; charset=utf-8\r\nContent-Length: {}\r\nSet-Cookie: private=fixture\r\n", body.len()), body.as_bytes()), false).await;
        let target = server.target("document-reader.invalid");
        let dns = Arc::new(AtomicUsize::new(0));
        let result = fetch_document(&target.document, server.client(&target, true, dns.clone()).build().unwrap()).await.unwrap();
        assert_eq!(result["text"], body);
        assert_eq!(result["byte_count"], body.len());
        assert_eq!(result["sha256"], format!("{:x}", Sha256::digest(body.as_bytes())));
        assert!(result["requested_url"].as_str().unwrap().ends_with("#section"));
        assert!(!result["document_url"].as_str().unwrap().contains('#'));
        assert!(!result.to_string().contains("private=fixture"));
        // A second fresh connector to the same host does not inherit the
        // first response's cookie. Production likewise builds per read.
        let second = fetch_document(&target.document, server.client(&target, true, dns.clone()).build().unwrap()).await.unwrap();
        assert_eq!(second, result);
        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].0.starts_with("GET /guide?lang=en HTTP/1.1\r\n"));
        assert_eq!(requests[0].1, "document-reader.invalid");
        let request = requests[0].0.to_ascii_lowercase();
        assert!(request.contains(&format!("host: document-reader.invalid:{}\r\n", server.address.port())));
        for absent in ["authorization:", "proxy-authorization:", "cookie:", "referer:", "#section"] {
            assert!(!request.contains(absent));
            assert!(!requests[1].0.to_ascii_lowercase().contains(absent));
        }
        assert!(request.contains("accept-encoding: identity\r\n"));
        assert_eq!(dns.load(Ordering::SeqCst), 0, "pinned hostname cannot fall back to DNS");
        server.finish().await;
    }

    #[tokio::test]
    async fn pinned_transport_rejects_untrusted_certificate_and_wrong_hostname() {
        for (host, trust) in [("document-reader.invalid", false), ("other.invalid", true)] {
            let server = Server::start(response("200 OK", "Content-Type: text/plain\r\nContent-Length: 0\r\n", b""), false).await;
            let target = server.target(host);
            let dns = Arc::new(AtomicUsize::new(0));
            assert!(matches!(fetch_document(&target.document, server.client(&target, trust, dns.clone()).build().unwrap()).await, Err(Refusal::Transport)));
            assert!(server.requests.lock().unwrap().is_empty());
            assert_eq!(dns.load(Ordering::SeqCst), 0);
            server.finish().await;
        }
    }

    #[tokio::test]
    async fn redirects_and_incomplete_or_unsupported_documents_do_not_return_source() {
        let oversized = vec![b'x'; MAX_BODY + 1];
        for (reply, expected) in [
            (response("302 Found", "Location: https://elsewhere.invalid:8443/next?q=1#part\r\nContent-Length: 0\r\n", b""), "redirect_refused"),
            (response("206 Partial Content", "Content-Type: text/plain\r\nContent-Length: 1\r\n", b"x"), "http_status"),
            (response("200 OK", "Content-Type: application/octet-stream\r\nContent-Length: 1\r\n", b"x"), "unsupported_media_type"),
            (response("200 OK", "Content-Type: text/plain; charset=latin1\r\nContent-Length: 1\r\n", b"x"), "unsupported_encoding"),
            (response("200 OK", "Content-Type: text/plain\r\nContent-Encoding: gzip\r\nContent-Length: 1\r\n", b"x"), "unsupported_encoding"),
            (response("200 OK", "Content-Type: text/plain\r\nContent-Length: 1\r\n", &[0xff]), "invalid_utf8"),
            (response("200 OK", "Content-Type: text/plain\r\n", &oversized), "document_too_large"),
        ] {
            let server = Server::start(reply, false).await;
            let target = server.target("document-reader.invalid");
            let error = fetch_document(&target.document, server.client(&target, true, Arc::new(AtomicUsize::new(0))).build().unwrap()).await.unwrap_err().value();
            assert_eq!(error["error"], expected);
            assert!(error.get("text").is_none());
            if expected == "redirect_refused" {
                assert_eq!(error["suggested_url"], "https://elsewhere.invalid:8443/next?q=1#part");
            }
            assert_eq!(server.requests.lock().unwrap().len(), 1);
            server.finish().await;
        }
    }

    #[tokio::test]
    async fn timeout_and_dropped_read_close_the_entered_request_without_retry() {
        for cancel in [false, true] {
            let server = Server::start(response("200 OK", "Content-Type: text/plain\r\nContent-Length: 10\r\n", b""), true).await;
            let target = server.target("document-reader.invalid");
            let builder = server.client(&target, true, Arc::new(AtomicUsize::new(0)));
            let client = if cancel { builder } else { builder.timeout(Duration::from_millis(100)) }.build().unwrap();
            let mut read = Box::pin(fetch_document(&target.document, client));
            tokio::select! {
                result = &mut read => panic!("read settled before fixture hold: {result:?}"),
                _ = server.seen.notified() => (),
            }
            if cancel { drop(read); } else { assert!(matches!(read.await, Err(Refusal::Timeout))); }
            timeout(BOUND, server.closed.notified()).await.unwrap();
            assert_eq!(server.requests.lock().unwrap().len(), 1);
            server.finish().await;
        }
    }
}
