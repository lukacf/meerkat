//! Templates applied to a request: the binding an identity seals, the tool
//! policy a template derives, and the preamble it composes.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::config::PreparationMode;
use super::digest::{Sha256Digest, canonical_digest};
use super::policy::{ModelChoice, ProfileChoice, Template};
use super::refusal::IdentityRefusalKind;

const FORMAT_BINDING: &str = "meerkat.identity.template_binding.v1";
const FORMAT_CALLBACK: &str = "meerkat.identity.callback.v1";
const FORMAT_PROFILE: &str = "meerkat.identity.profile.v1";

/// What a template binding seals of one mob profile.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ProfileFacts {
    /// The profile's model.
    pub model: String,
    /// The profile's digest: any change of the profile changes it.
    pub digest: Sha256Digest,
}

impl ProfileFacts {
    /// Facts from a model and a digest the caller computed.
    pub fn new(model: impl Into<String>, digest: Sha256Digest) -> Self {
        Self {
            model: model.into(),
            digest,
        }
    }

    /// The facts of an inline mob profile. The digest is over the profile's
    /// serialized form with null, false, empty-text, empty-list and
    /// empty-map members left out, so a Core release that adds a profile
    /// field which serializes to one of those keeps every digest. A field
    /// added with any other default changes them, and the identities whose
    /// template binds the profile then read Broken until the host rebinds
    /// them.
    pub fn of_profile(profile: &meerkat_mob::Profile) -> Result<Self, serde_json::Error> {
        let value = serde_json::to_value(profile)?;
        Ok(Self {
            model: profile.model.clone(),
            digest: canonical_digest(&json!({
                "format": FORMAT_PROFILE,
                "profile": without_empty_members(value),
            })),
        })
    }
}

/// `value` with every object member whose value is null, false, an empty
/// string, an empty array or an empty object left out, at every depth.
fn without_empty_members(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let kept: Map<String, Value> = map
                .into_iter()
                .map(|(key, item)| (key, without_empty_members(item)))
                .filter(|(_, item)| !is_empty_member(item))
                .collect();
            Value::Object(kept)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(without_empty_members).collect()),
        other => other,
    }
}

fn is_empty_member(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => true,
        Value::String(text) => text.is_empty(),
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        Value::Bool(true) | Value::Number(_) => false,
    }
}

/// The mob profiles a template may bind, by name.
pub type MobProfiles = BTreeMap<String, ProfileFacts>;

/// The digest of a callback tool's definition: its name, description and
/// input schema, as a template declares them and as a host's build gives
/// them (absent members digest as absent). A template binding seals one per
/// callback the template declares, so a changed declaration changes the
/// binding of every identity created from the template.
pub fn callback_definition_digest(
    name: &str,
    description: Option<&str>,
    input_schema: Option<&Value>,
) -> Sha256Digest {
    canonical_digest(&json!({
        "format": FORMAT_CALLBACK,
        "name": name,
        "description": description,
        "input_schema": input_schema,
    }))
}

/// The build-time check of a created identity's callback tools: the
/// definitions a host's build gives (each tool's name, description and
/// input schema, as its `callback/build_agent` answer carries them) must be
/// exactly the ones the binding sealed. A changed, missing, extra or
/// repeated tool reads `template_binding_changed`: the identity is Broken
/// until a host rebind. The executor's build path calls it; this module
/// only defines it.
pub fn check_build_callbacks<'a>(
    binding: &TemplateBinding,
    supplied: impl IntoIterator<Item = (&'a str, Option<&'a str>, Option<&'a Value>)>,
) -> Result<(), IdentityRefusalKind> {
    let mut built = BTreeMap::new();
    for (name, description, input_schema) in supplied {
        let digest = callback_definition_digest(name, description, input_schema);
        if built.insert(name.to_owned(), digest).is_some() {
            return Err(IdentityRefusalKind::TemplateBindingChanged);
        }
    }
    if built == binding.callbacks {
        Ok(())
    } else {
        Err(IdentityRefusalKind::TemplateBindingChanged)
    }
}

/// The exact template binding an identity seals for its life: the template
/// and its install version, the bound profile and its digest, the model,
/// the accounts, the declared definitions of its callback tools, the tool
/// floor and ceiling, and the preparation rule. Its digest is what every
/// build compares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct TemplateBinding {
    /// The template's id.
    pub template: String,
    /// The template's install version.
    pub version: Option<String>,
    /// The bound profile.
    pub profile: String,
    /// The bound profile's digest.
    pub profile_digest: Sha256Digest,
    /// The model.
    pub model: String,
    /// The account binding references.
    pub accounts: BTreeSet<String>,
    /// The callback tools the template declares, by name: the digest of
    /// each one's definition (`callback_definition_digest`).
    pub callbacks: BTreeMap<String, Sha256Digest>,
    /// The tool floor.
    pub floor: BTreeSet<String>,
    /// The tool ceiling.
    pub ceiling: BTreeSet<String>,
    /// The preparation rule.
    pub preparation: PreparationMode,
}

impl TemplateBinding {
    /// The binding's digest. Presentation fields (title, description) and
    /// fields that act at each build or only at creation (preamble, limits,
    /// kickoff rule, namespace, excess mode, authority field declarations)
    /// are not in it.
    pub fn digest(&self) -> Sha256Digest {
        canonical_digest(&json!({
            "format": FORMAT_BINDING,
            "template": self.template,
            "version": self.version,
            "profile": { "name": self.profile, "digest": self.profile_digest.as_str() },
            "model": self.model,
            "accounts": self.accounts,
            "callbacks": self.callbacks,
            "tools": { "floor": self.floor, "ceiling": self.ceiling },
            "preparation": self.preparation.token(),
        }))
    }
}

/// Resolve a request's profile and model against `template` and the mob's
/// profiles, into the binding the identity seals. A request that names no
/// profile takes the template's first listed one; one that names no model
/// takes the template's first listed model, or the profile's own.
pub fn resolve_binding(
    template: &Template,
    profile: Option<&str>,
    model: Option<&str>,
    profiles: &MobProfiles,
) -> Result<TemplateBinding, IdentityRefusalKind> {
    let profile = match (profile, template.profiles()) {
        (Some(name), ProfileChoice::Any) => name,
        (Some(name), ProfileChoice::Listed(listed)) if listed.iter().any(|item| item == name) => {
            name
        }
        (Some(name), ProfileChoice::Listed(_)) => {
            return Err(IdentityRefusalKind::ProfileNotAllowed {
                profile: Some(name.to_owned()),
            });
        }
        (None, ProfileChoice::Listed(listed)) => match listed.first() {
            Some(first) => first.as_str(),
            None => return Err(IdentityRefusalKind::ProfileNotAllowed { profile: None }),
        },
        (None, ProfileChoice::Any) => {
            return Err(IdentityRefusalKind::ProfileNotAllowed { profile: None });
        }
    };
    let Some(facts) = profiles.get(profile) else {
        return Err(IdentityRefusalKind::ProfileNotAllowed {
            profile: Some(profile.to_owned()),
        });
    };
    let model = match (model, template.models()) {
        (Some(name), ModelChoice::ProfileModel) if name == facts.model => name,
        (Some(name), ModelChoice::Listed(listed)) if listed.iter().any(|item| item == name) => name,
        (Some(name), _) => {
            return Err(IdentityRefusalKind::ModelNotAllowed {
                model: name.to_owned(),
            });
        }
        (None, ModelChoice::ProfileModel) => facts.model.as_str(),
        (None, ModelChoice::Listed(listed)) => match listed.first() {
            Some(first) => first.as_str(),
            None => {
                return Err(IdentityRefusalKind::ModelNotAllowed {
                    model: String::new(),
                });
            }
        },
    };
    Ok(TemplateBinding {
        template: template.id().to_owned(),
        version: template.version().map(str::to_owned),
        profile: profile.to_owned(),
        profile_digest: facts.digest.clone(),
        model: model.to_owned(),
        accounts: template.accounts().clone(),
        callbacks: template.callbacks().clone(),
        floor: template.floor().clone(),
        ceiling: template.ceiling().clone(),
        preparation: template.preparation(),
    })
}

/// What the creator side contributes to a created identity's tools, at a
/// build or a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreatorBound<'a> {
    /// A host or human creator: no creator bound.
    Unbounded,
    /// An agent creator, resolved: its tools now.
    Live(&'a BTreeSet<String>),
    /// An agent creator that cannot be resolved, or was removed. It
    /// contributes nothing, so creator loss never widens the tools and
    /// never restores a revocation.
    Unresolved,
}

/// A created identity's effective tools: `(sealed & ceiling & creator) |
/// (sealed & floor) | (sealed & approved_excess)`. `sealed` is the tools
/// its accepted request named; `approved_excess` the excess items a human
/// decision approved, which are constant for the identity's life. For a
/// host or human creator the creator part is `sealed & ceiling`.
pub fn effective_tools(
    binding: &TemplateBinding,
    sealed: &BTreeSet<String>,
    approved_excess: &BTreeSet<String>,
    creator: CreatorBound<'_>,
) -> BTreeSet<String> {
    let mut tools: BTreeSet<String> = match creator {
        CreatorBound::Unbounded => sealed.intersection(&binding.ceiling).cloned().collect(),
        CreatorBound::Live(live) => sealed
            .iter()
            .filter(|tool| binding.ceiling.contains(*tool) && live.contains(*tool))
            .cloned()
            .collect(),
        CreatorBound::Unresolved => BTreeSet::new(),
    };
    tools.extend(sealed.intersection(&binding.floor).cloned());
    tools.extend(sealed.intersection(approved_excess).cloned());
    tools
}

/// The most a created identity can ever use under its sealed request: its
/// effective tools with an unbounded creator. Every build may set it as the
/// member's native tool access policy, so a build that applies no per-call
/// bound still stays within it.
pub fn sealed_upper_bound(
    binding: &TemplateBinding,
    sealed: &BTreeSet<String>,
    approved_excess: &BTreeSet<String>,
) -> BTreeSet<String> {
    effective_tools(binding, sealed, approved_excess, CreatorBound::Unbounded)
}

/// `tools` as Core's native tool access policy: an allow list.
pub fn native_tool_policy(tools: &BTreeSet<String>) -> meerkat_core::ToolAccessPolicy {
    meerkat_core::ToolAccessPolicy::AllowList(tools.iter().map(String::as_str).collect())
}

/// One part of a parsed preamble.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PreambleSegment {
    Text(String),
    Creator,
    Peers,
    DisplayName,
    Purpose,
}

/// A template's host preamble, parsed: literal text and the placeholders
/// `{creator}`, `{peers}`, `{display_name}` and `{purpose}`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preamble {
    segments: Vec<PreambleSegment>,
}

/// What a preamble's placeholders are filled with at a build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreambleInputs<'a> {
    /// The creator, as the member should read it.
    pub creator: &'a str,
    /// The member's current peers.
    pub peers: &'a [String],
    /// The member's display name.
    pub display_name: &'a str,
    /// The member's purpose.
    pub purpose: &'a str,
}

/// The heading of the creator's instructions in a created member's
/// instructions.
pub const CREATOR_INSTRUCTIONS_HEADING: &str = "Instructions from your creator:";

impl Preamble {
    /// Parse host preamble text. `{{` and `}}` are literal braces; any other
    /// brace must open or close one of the four placeholders. The error is
    /// the offending text.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut segments = Vec::new();
        let mut literal = String::new();
        let mut rest = text;
        while let Some(character) = rest.chars().next() {
            if let Some(after) = rest.strip_prefix("{{") {
                literal.push('{');
                rest = after;
            } else if let Some(after) = rest.strip_prefix("}}") {
                literal.push('}');
                rest = after;
            } else if let Some(after) = rest.strip_prefix('{') {
                let Some(close) = after.find('}') else {
                    return Err(rest.to_owned());
                };
                let name = after.get(..close).unwrap_or_default();
                let segment = match name {
                    "creator" => PreambleSegment::Creator,
                    "peers" => PreambleSegment::Peers,
                    "display_name" => PreambleSegment::DisplayName,
                    "purpose" => PreambleSegment::Purpose,
                    _ => return Err(format!("{{{name}}}")),
                };
                if !literal.is_empty() {
                    segments.push(PreambleSegment::Text(std::mem::take(&mut literal)));
                }
                segments.push(segment);
                rest = after.get(close + 1..).unwrap_or_default();
            } else if character == '}' {
                return Err("}".to_owned());
            } else {
                literal.push(character);
                rest = rest.get(character.len_utf8()..).unwrap_or_default();
            }
        }
        if !literal.is_empty() {
            segments.push(PreambleSegment::Text(literal));
        }
        Ok(Self { segments })
    }

    /// Whether the preamble has no text and no placeholder.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The preamble with its placeholders filled. `{peers}` is the peers in
    /// byte order, joined with `", "`, or `none`.
    pub fn render(&self, inputs: &PreambleInputs<'_>) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            match segment {
                PreambleSegment::Text(text) => out.push_str(text),
                PreambleSegment::Creator => out.push_str(inputs.creator),
                PreambleSegment::Peers => out.push_str(&peer_list(inputs.peers)),
                PreambleSegment::DisplayName => out.push_str(inputs.display_name),
                PreambleSegment::Purpose => out.push_str(inputs.purpose),
            }
        }
        out
    }
}

fn peer_list(peers: &[String]) -> String {
    let sorted: BTreeSet<&str> = peers.iter().map(String::as_str).collect();
    if sorted.is_empty() {
        "none".to_owned()
    } else {
        sorted.into_iter().collect::<Vec<_>>().join(", ")
    }
}

/// A created member's instructions at a build: the template's preamble with
/// its placeholders filled, as host authority, then the creator's
/// instructions under their own heading.
pub fn compose_member_instructions(
    preamble: &Preamble,
    inputs: &PreambleInputs<'_>,
    instructions: &str,
) -> String {
    let mut out = preamble.render(inputs);
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(CREATOR_INSTRUCTIONS_HEADING);
    out.push('\n');
    out.push_str(instructions);
    out
}
