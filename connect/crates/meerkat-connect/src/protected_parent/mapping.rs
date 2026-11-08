//! Closed, bounded protected service mapping syntax. Parsing authenticates no
//! principal, selects no runtime, opens no path and supplies no ambient default.
//! The original sealed bytes remain with the caller for their mapping digest.
use meerkat_authorization_contracts::constraints::ResourceDomain;
use meerkat_core::{PrincipalKind, PrincipalRef, RealmId, TrustDomainId};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use std::{collections::BTreeSet, fmt, path::PathBuf};

type ServiceGroups = Vec<(String, std::ops::Range<usize>)>;

const SCHEMA: &str = "meerkat.connect.protected-service.v1";
const MAX_BYTES: usize = 65536;
const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 16384; // Values and object keys, counted before conversion.
const MAX_PRINCIPAL_MAPPINGS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MappingRefused;
impl fmt::Display for MappingRefused {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("Protected service mapping refused")
    }
}
impl std::error::Error for MappingRefused {}

#[derive(Debug)]
pub(super) struct ParsedMapping {
    pub(super) application: ApplicationMapping,
    pub(super) source_selection: String,
    pub(super) principal_mappings: Vec<PrincipalMapping>,
    pub(super) authentication_evidence_domain: ResourceDomain,
    pub(super) realm: RealmMapping,
    pub(super) ingress: PrincipalRef,
    pub(super) installation: InstallationMapping,
    pub(super) execution: ExecutionMapping,
    pub(super) provider_store: ProviderStore,
    pub(super) catalog_id: String,
    pub(super) collections: Vec<CollectionDescriptor>,
    pub(super) service_groups: ServiceGroups,
    pub(super) selected_input: Option<super::selected_input::Identity>,
}
#[derive(Debug)]
pub(super) struct ApplicationMapping {
    pub(super) application_id: String,
    pub(super) origin: String,
}
#[derive(Debug)]
pub(super) struct PrincipalMapping {
    pub(super) issuer: String,
    pub(super) subject: String,
    pub(super) workspace: String,
    pub(super) origin: String,
    pub(super) caller: PrincipalRef,
}
#[derive(Debug)]
pub(super) struct RealmMapping {
    pub(super) realm_id: RealmId,
    pub(super) state_root: PathBuf,
    pub(super) context_root: PathBuf,
    pub(super) user_config_root: PathBuf,
}
#[derive(Debug)]
pub(super) struct InstallationMapping {
    pub(super) executable: PathBuf,
    pub(super) resource_root: PathBuf,
    pub(super) resource_manifest_sha256: [u8; 32],
}
#[derive(Debug)]
pub(super) struct ExecutionMapping {
    pub(super) uid: u32,
    pub(super) gid: u32,
    pub(super) supplementary_gids: Vec<u32>,
}
#[derive(Debug)]
pub(super) enum ProviderStore {
    File {
        root: PathBuf,
    },
    // The entry owner must match actual HOME/XDG and pinned Linux Core
    // default_auto File resolution, then pass Some(root) explicitly. The
    // concrete entry also requires a precreated child-UID-owned 0700 root.
    // This parser neither inspects that directory nor proves isolation.
    NativeDefault {
        home: PathBuf,
        xdg_config_home: Option<PathBuf>,
        root: PathBuf,
    },
}

/// Validated descriptors only. The compiled catalog owns program lookup and
/// exact resource/operation compatibility. These types carry no native owner.
#[derive(Debug)]
pub(super) struct CollectionDescriptor {
    pub(super) service_id: String,
    pub(super) source_id: String,
    pub(super) family: String,
    pub(super) profile: String,
    pub(super) agent: String,
    pub(super) requirements: Vec<RequirementDescriptor>,
    pub(super) limits: CollectionLimitDescriptor,
}
#[derive(Debug)]
pub(super) struct RequirementDescriptor {
    pub(super) id: String,
    pub(super) program_id: String,
    pub(super) resource: Value,
    pub(super) raw_operation_ceiling: Vec<String>,
}
#[derive(Debug)]
pub(super) struct CollectionLimitDescriptor {
    pub(super) calls: usize,
    pub(super) argument_bytes: usize,
    pub(super) result_bytes: usize,
}

impl ParsedMapping {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, MappingRefused> {
        let value = bounded_value(bytes)?;
        let schema = value
            .get("schema")
            .and_then(Value::as_str)
            .ok_or(MappingRefused)?;
        let selected = schema == "meerkat.connect.protected-service.v3";
        let multiple = match schema {
            SCHEMA => false,
            "meerkat.connect.protected-service.v2" | "meerkat.connect.protected-service.v3" => true,
            _ => return Err(MappingRefused),
        };
        let selection_field = if multiple { "services" } else { "collections" };
        let mut fields = vec![
            "schema",
            "protocol_version",
            "application",
            "source_selection",
            "principal_mappings",
            "authentication_evidence_domain",
            "realm",
            "ingress",
            "installation",
            "execution",
            "provider_store",
            "catalog_id",
            selection_field,
        ];
        if selected {
            fields.push("selected_input");
        }
        let mut root = object(value, &fields)?;
        let _schema = text(take(&mut root, "schema")?, 256)?;
        if unsigned(take(&mut root, "protocol_version")?)? != 7 {
            return Err(MappingRefused);
        }
        let mut app = object(
            take(&mut root, "application")?,
            &["application_id", "origin"],
        )?;
        let application = ApplicationMapping {
            application_id: text(take(&mut app, "application_id")?, 256)?,
            origin: text(take(&mut app, "origin")?, 2048)?,
        };
        let source_selection = text(take(&mut root, "source_selection")?, 256)?;
        let ingress = principal(take(&mut root, "ingress")?)?;
        let mut domain = object(
            take(&mut root, "authentication_evidence_domain")?,
            &["authority", "namespace"],
        )?;
        let authentication_evidence_domain = ResourceDomain {
            authority: principal(take(&mut domain, "authority")?)?,
            namespace: text(take(&mut domain, "namespace")?, 256)?,
        };
        let mut principal_mappings = Vec::new();
        let mut identities = BTreeSet::new();
        for row in array(
            take(&mut root, "principal_mappings")?,
            1,
            MAX_PRINCIPAL_MAPPINGS,
        )? {
            let mut row = object(row, &["issuer", "subject", "workspace", "origin", "caller"])?;
            let issuer = text(take(&mut row, "issuer")?, 1024)?;
            let subject = text(take(&mut row, "subject")?, 256)?;
            let workspace = hex_id(take(&mut row, "workspace")?)?;
            let origin = text(take(&mut row, "origin")?, 2048)?;
            let caller = principal(take(&mut row, "caller")?)?;
            if origin != application.origin
                || caller == ingress
                || !identities.insert((
                    issuer.clone(),
                    subject.clone(),
                    workspace.clone(),
                    origin.clone(),
                ))
            {
                return Err(MappingRefused);
            }
            principal_mappings.push(PrincipalMapping {
                issuer,
                subject,
                workspace,
                origin,
                caller,
            });
        }
        let mut realm = object(
            take(&mut root, "realm")?,
            &["realm_id", "state_root", "context_root", "user_config_root"],
        )?;
        let realm = RealmMapping {
            realm_id: RealmId::parse(text(take(&mut realm, "realm_id")?, 256)?)
                .map_err(|_| MappingRefused)?,
            state_root: path(take(&mut realm, "state_root")?)?,
            context_root: path(take(&mut realm, "context_root")?)?,
            user_config_root: path(take(&mut realm, "user_config_root")?)?,
        };
        let mut installation = object(
            take(&mut root, "installation")?,
            &["executable", "resource_root", "resource_manifest_sha256"],
        )?;
        let installation = InstallationMapping {
            executable: path(take(&mut installation, "executable")?)?,
            resource_root: path(take(&mut installation, "resource_root")?)?,
            resource_manifest_sha256: digest(take(&mut installation, "resource_manifest_sha256")?)?,
        };
        let mut execution = object(
            take(&mut root, "execution")?,
            &["uid", "gid", "supplementary_gids"],
        )?;
        let uid = linux_id(take(&mut execution, "uid")?)?;
        let gid = linux_id(take(&mut execution, "gid")?)?;
        if uid == 0 || gid == 0 {
            return Err(MappingRefused);
        }
        let mut supplementary_gids = Vec::new();
        for raw in array(take(&mut execution, "supplementary_gids")?, 0, 64)? {
            let group = linux_id(raw)?;
            if group == 0
                || supplementary_gids
                    .last()
                    .is_some_and(|previous| *previous >= group)
            {
                return Err(MappingRefused);
            }
            supplementary_gids.push(group);
        }
        let execution = ExecutionMapping {
            uid,
            gid,
            supplementary_gids,
        };
        let provider_store = provider_store(take(&mut root, "provider_store")?)?;
        let catalog_id = text(take(&mut root, "catalog_id")?, 256)?;
        let (collections, service_groups) = if multiple {
            service_groups(take(&mut root, "services")?)?
        } else {
            let mut rows = collections(take(&mut root, "collections")?)?;
            for row in &mut rows {
                row.service_id = "default".into();
            }
            let length = rows.len();
            (rows, vec![("default".into(), 0..length)])
        };
        let selected_input = if selected {
            let mut row = object(
                take(&mut root, "selected_input")?,
                &["service_id", "bundle_sha256"],
            )?;
            let service_id = text(take(&mut row, "service_id")?, 256)?;
            if !service_groups.iter().any(|(id, _)| id == &service_id) {
                return Err(MappingRefused);
            }
            Some(super::selected_input::Identity {
                service_id,
                bundle_sha256: hex_id(take(&mut row, "bundle_sha256")?)?,
            })
        } else {
            None
        };
        Ok(Self {
            application,
            source_selection,
            principal_mappings,
            authentication_evidence_domain,
            realm,
            ingress,
            installation,
            execution,
            provider_store,
            catalog_id,
            collections,
            service_groups,
            selected_input,
        })
    }
}

fn service_groups(
    value: Value,
) -> Result<(Vec<CollectionDescriptor>, ServiceGroups), MappingRefused> {
    let mut result = Vec::new();
    let mut groups = Vec::new();
    let mut ids = BTreeSet::new();
    let mut sources = BTreeSet::new();
    let mut operations = BTreeSet::new();
    let mut requirements = 0;
    for value in array(value, 1, 16)? {
        let mut row = object(value, &["service_id", "collections"])?;
        let id = text(take(&mut row, "service_id")?, 256)?;
        if !ids.insert(id.clone()) {
            return Err(MappingRefused);
        }
        let mut selected = collections(take(&mut row, "collections")?)?;
        for source in &mut selected {
            if !sources.insert(source.source_id.clone()) {
                return Err(MappingRefused);
            }
            source.service_id = id.clone();
            requirements += source.requirements.len();
            for requirement in &source.requirements {
                operations.extend(requirement.raw_operation_ceiling.iter().cloned());
            }
        }
        if sources.len() > 16 || requirements > 256 || operations.len() > 256 {
            return Err(MappingRefused);
        }
        let start = result.len();
        result.extend(selected);
        groups.push((id, start..result.len()));
    }
    Ok((result, groups))
}

fn collections(value: Value) -> Result<Vec<CollectionDescriptor>, MappingRefused> {
    let mut result: Vec<CollectionDescriptor> = Vec::new();
    let mut source_ids = BTreeSet::new();
    let mut all_operations = BTreeSet::new();
    let mut total_requirements = 0;
    for value in array(value, 1, 16)? {
        let mut row = object(
            value,
            &[
                "source_id",
                "family",
                "profile",
                "agent",
                "requirements",
                "limits",
            ],
        )?;
        let source_id = text(take(&mut row, "source_id")?, 256)?;
        let family = text(take(&mut row, "family")?, 256)?;
        let profile = text(take(&mut row, "profile")?, 256)?;
        let agent = text(take(&mut row, "agent")?, 256)?;
        if !source_ids.insert(source_id.clone())
            || result.first().is_some_and(|first| {
                (&first.family, &first.profile, &first.agent) != (&family, &profile, &agent)
            })
        {
            return Err(MappingRefused);
        }
        let mut requirements = Vec::new();
        let mut ids = BTreeSet::new();
        for value in array(take(&mut row, "requirements")?, 1, 64)? {
            let mut requirement = object(
                value,
                &["id", "program_id", "resource", "raw_operation_ceiling"],
            )?;
            let id = text(take(&mut requirement, "id")?, 256)?;
            let program_id = text(take(&mut requirement, "program_id")?, 256)?;
            if !ids.insert(id.clone()) {
                return Err(MappingRefused);
            }
            let resource = take(&mut requirement, "resource")?;
            // Match RequiredCollectionOperation's serialized resource budget.
            // This rendering is never used for the original mapping digest.
            if resource.is_null() || resource.to_string().len() > 4096 {
                return Err(MappingRefused);
            }
            let mut raw_operation_ceiling = Vec::new();
            let mut operations = BTreeSet::new();
            for operation in array(take(&mut requirement, "raw_operation_ceiling")?, 1, 32)? {
                let operation = text(operation, 256)?;
                if !operations.insert(operation.clone()) {
                    return Err(MappingRefused);
                }
                all_operations.insert(operation.clone());
                raw_operation_ceiling.push(operation);
            }
            total_requirements += 1;
            if total_requirements > 256 || all_operations.len() > 256 {
                return Err(MappingRefused);
            }
            requirements.push(RequirementDescriptor {
                id,
                program_id,
                resource,
                raw_operation_ceiling,
            });
        }
        let mut limits = object(
            take(&mut row, "limits")?,
            &["calls", "argument_bytes", "result_bytes"],
        )?;
        let limits = CollectionLimitDescriptor {
            calls: bounded_unsigned(take(&mut limits, "calls")?, 256)?,
            argument_bytes: bounded_unsigned(
                take(&mut limits, "argument_bytes")?,
                8 * 1024 * 1024,
            )?,
            result_bytes: bounded_unsigned(take(&mut limits, "result_bytes")?, 32 * 1024 * 1024)?,
        };
        result.push(CollectionDescriptor {
            service_id: String::new(),
            source_id,
            family,
            profile,
            agent,
            requirements,
            limits,
        });
    }
    Ok(result)
}
fn principal(value: Value) -> Result<PrincipalRef, MappingRefused> {
    let mut row = object(value, &["kind", "id", "qualification"])?;
    let kind = text(take(&mut row, "kind")?, 256)?;
    let kind: PrincipalKind =
        serde_json::from_value(Value::String(kind)).map_err(|_| MappingRefused)?;
    let id = text(take(&mut row, "id")?, 256)?;
    let mut qualification = object(
        take(&mut row, "qualification")?,
        &["kind", "trust_domain_id"],
    )?;
    if text(take(&mut qualification, "kind")?, 256)? != "qualified" {
        return Err(MappingRefused);
    }
    let domain = TrustDomainId::new(text(take(&mut qualification, "trust_domain_id")?, 256)?)
        .map_err(|_| MappingRefused)?;
    PrincipalRef::in_domain(kind, id, domain).map_err(|_| MappingRefused)
}
fn provider_store(value: Value) -> Result<ProviderStore, MappingRefused> {
    let Value::Object(mut row) = value else {
        return Err(MappingRefused);
    };
    let kind = text(take(&mut row, "kind")?, 256)?;
    match kind.as_str() {
        "native_default"
            if row.len() == 3
                && row.contains_key("home")
                && row.contains_key("xdg_config_home")
                && row.contains_key("root") =>
        {
            let home = path(take(&mut row, "home")?)?;
            let xdg_config_home = match take(&mut row, "xdg_config_home")? {
                Value::Null => None,
                value => Some(path(value)?),
            };
            let root = path(take(&mut row, "root")?)?;
            Ok(ProviderStore::NativeDefault {
                home,
                xdg_config_home,
                root,
            })
        }
        "file" if row.len() == 1 && row.contains_key("root") => Ok(ProviderStore::File {
            root: path(take(&mut row, "root")?)?,
        }),
        _ => Err(MappingRefused),
    }
}
fn object(value: Value, fields: &[&str]) -> Result<Map<String, Value>, MappingRefused> {
    let Value::Object(row) = value else {
        return Err(MappingRefused);
    };
    if row.len() != fields.len() || row.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err(MappingRefused);
    }
    Ok(row)
}
fn take(row: &mut Map<String, Value>, key: &str) -> Result<Value, MappingRefused> {
    row.remove(key).ok_or(MappingRefused)
}
fn array(value: Value, min: usize, max: usize) -> Result<Vec<Value>, MappingRefused> {
    match value {
        Value::Array(values) if (min..=max).contains(&values.len()) => Ok(values),
        _ => Err(MappingRefused),
    }
}
fn text(value: Value, max: usize) -> Result<String, MappingRefused> {
    match value {
        Value::String(value)
            if !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control) =>
        {
            Ok(value)
        }
        _ => Err(MappingRefused),
    }
}
fn unsigned(value: Value) -> Result<u64, MappingRefused> {
    value.as_u64().ok_or(MappingRefused)
}
fn bounded_unsigned(value: Value, max: u64) -> Result<usize, MappingRefused> {
    let value = unsigned(value)?;
    if !(1..=max).contains(&value) {
        return Err(MappingRefused);
    }
    usize::try_from(value).map_err(|_| MappingRefused)
}
fn linux_id(value: Value) -> Result<u32, MappingRefused> {
    u32::try_from(unsigned(value)?).map_err(|_| MappingRefused)
}
fn hex_id(value: Value) -> Result<String, MappingRefused> {
    let value = text(value, 64)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(MappingRefused);
    }
    Ok(value)
}
fn digest(value: Value) -> Result<[u8; 32], MappingRefused> {
    let value = hex_id(value)?;
    let mut result = [0; 32];
    for (out, pair) in result.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        let digit = |byte: u8| {
            if byte.is_ascii_digit() {
                byte - b'0'
            } else {
                byte - b'a' + 10
            }
        };
        *out = digit(pair[0]) * 16 + digit(pair[1]);
    }
    Ok(result)
}
fn path(value: Value) -> Result<PathBuf, MappingRefused> {
    let value = text(value, 4096)?;
    // Check spelling before Path::components could discard '.' or repeated '/'.
    if !value.starts_with('/')
        || value != "/"
            && value[1..]
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(MappingRefused);
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(MappingRefused);
    }
    Ok(path)
}

// A bounded syntax traversal rejects decoded duplicate keys at every depth,
// including free-form resources, before ordinary Value/native conversion. The
// second parse consumes the same original bytes, preserving serde_json's number
// behavior even when another crate enables its arbitrary_precision feature.
fn bounded_value(bytes: &[u8]) -> Result<Value, MappingRefused> {
    bounded_json(bytes, MAX_BYTES, MAX_DEPTH, MAX_NODES)
}
pub(super) fn bounded_json(
    bytes: &[u8],
    max_bytes: usize,
    max_depth: usize,
    max_nodes: usize,
) -> Result<Value, MappingRefused> {
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(MappingRefused);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let mut remaining = max_nodes;
    BoundedValue {
        depth: 0,
        max_depth,
        remaining: &mut remaining,
    }
    .deserialize(&mut deserializer)
    .map_err(|_| MappingRefused)?;
    deserializer.end().map_err(|_| MappingRefused)?;
    serde_json::from_slice(bytes).map_err(|_| MappingRefused)
}
struct BoundedValue<'a> {
    depth: usize,
    max_depth: usize,
    remaining: &'a mut usize,
}
impl<'de> DeserializeSeed<'de> for BoundedValue<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        if self.depth > self.max_depth || *self.remaining == 0 {
            return Err(de::Error::custom("mapping budget"));
        }
        *self.remaining -= 1;
        deserializer.deserialize_any(self)
    }
}
impl<'de> Visitor<'de> for BoundedValue<'_> {
    type Value = ();
    fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("bounded JSON")
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<(), E> {
        if value.is_finite() {
            Ok(())
        } else {
            Err(de::Error::custom("non-finite JSON"))
        }
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_string<E: de::Error>(self, _: String) -> Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut values: A) -> Result<(), A::Error> {
        while values
            .next_element_seed(BoundedValue {
                depth: self.depth + 1,
                max_depth: self.max_depth,
                remaining: &mut *self.remaining,
            })?
            .is_some()
        {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> Result<(), A::Error> {
        let mut keys = BTreeSet::new();
        while let Some(key) = fields.next_key::<String>()? {
            if *self.remaining == 0 || !keys.insert(key) {
                return Err(de::Error::custom("mapping key budget or duplicate"));
            }
            *self.remaining -= 1;
            fields.next_value_seed(BoundedValue {
                depth: self.depth + 1,
                max_depth: self.max_depth,
                remaining: &mut *self.remaining,
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "mapping_tests.rs"]
mod tests;
