//! `render_v1`: the one canonical, complete plain-text rendering of a
//! normalized identity request. Its sha256 is the decision's subject digest.
//!
//! # Properties
//!
//! - **Total.** Every field of the normalized request is rendered, an absent
//!   one as `none` (or `unchanged` for an update field), so nothing is
//!   omitted.
//! - **Injective.** Every value is quoted and escaped reversibly (see
//!   `escape`), lists carry their count, and free text carries its line
//!   count, so the rendering determines the request it was made from.
//! - **Sorted.** Maps and sets render in byte order; nothing depends on an
//!   iteration order that could change.
//! - **Deterministic.** The same request and the same host consequence lines
//!   give the same bytes. The rendering holds no time of its own; a time is
//!   rendered only when the request carries one.
//! - **Never cut.** A text past its limit is refused at normalization
//!   (`text_too_large`), and a rendering past 256 KiB is refused here.
//!
//! # Layout
//!
//! Blocks of lines separated by one empty line; every line ends with `\n`.
//! A line is one of:
//!
//! - a field, `Label: value`, where a value is words and quoted strings
//!   separated by single spaces;
//! - a list, `Label (n):` followed by `n` lines `- value`;
//! - a text, `Label (n line):` or `Label (n lines):` followed by `n` lines
//!   `> escaped line` (or `>` for an empty line).
//!
//! The blocks come in this order: the header (the action, the requester,
//! the proposer and the source evidence, then the action's own fields);
//! the structural fields of each identity (template, binding, profile,
//! model, tools, peers, authority fields, descriptive labels); the
//! consequences the policy states; the host's consequence lines, in their
//! own block; then the free text of each identity, quoted line by line.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;

use serde_json::Value;

use super::config::ChangeClass;
use super::consequences::{ConsequenceHook, ConsequenceLine};
use super::digest::{Sha256Digest, canonical_json_pretty};
use super::escape::{escaped_line, quoted};
use super::parts::{CanonicalPart, canonical_parts};
use super::presentation::{CardError, StructuredCard};
use super::refusal::{IdentityRefusal, IdentityRefusalKind};
use super::request::{
    Consequence, ConsequenceEffect, CreatedSpec, EdgeBasis, IdentityRequest, RequestAction,
    Requester, UpdatedSpec,
};

/// The renderer token a decision records with a `render_v1` rendering.
pub const RENDERER: &str = "identity.render-v1";
/// The largest rendering, in bytes: the decision owner's limit.
pub const RENDERING_MAX_BYTES: usize = 256 * 1024;
/// The largest host consequence line, in bytes.
pub const HOST_CONSEQUENCE_MAX_BYTES: usize = 1024;
/// The largest set of host consequence lines, in bytes together.
pub const HOST_CONSEQUENCES_MAX_BYTES: usize = 64 * 1024;

/// The labels and words of the `render_v1` layout.
pub mod labels {
    /// The first line's label; its value is `VERSION`.
    pub const TITLE: &str = "Identity request";
    /// The rendering's version word.
    pub const VERSION: &str = "render-v1";
    /// The action: `create`, `update` or `remove`.
    pub const ACTION: &str = "Action";
    /// Who asks: `agent`, `host` or `human`, then a quoted name.
    pub const REQUESTER: &str = "Requester";
    /// Who proposed it.
    pub const PROPOSER: &str = "Proposer";
    /// Source evidence, when absent (`none`).
    pub const SOURCE_EVIDENCE: &str = "Source evidence";
    /// Source evidence: the attestor.
    pub const SOURCE_ATTESTOR: &str = "Source attestor";
    /// Source evidence: the subject.
    pub const SOURCE_SUBJECT: &str = "Source subject";
    /// Source evidence: the source reference.
    pub const SOURCE_REFERENCE: &str = "Source reference";
    /// Source evidence: when the source was observed, in UNIX milliseconds.
    pub const SOURCE_OBSERVED_AT_MS: &str = "Source observed at ms";
    /// How many identities a create names.
    pub const IDENTITIES: &str = "Identities";
    /// The identity an update or a remove names.
    pub const IDENTITY: &str = "Identity";
    /// The revision an update or a remove expects.
    pub const EXPECTED_REVISION: &str = "Expected revision";
    /// The template.
    pub const TEMPLATE: &str = "Template";
    /// The template binding's digest.
    pub const TEMPLATE_BINDING: &str = "Template binding";
    /// An update's new binding digest, or `none`.
    pub const TEMPLATE_REBIND: &str = "Template rebind";
    /// An update's change classes.
    pub const CHANGES: &str = "Changes";
    /// The heading of an update's structural fields.
    pub const UPDATE_OF: &str = "Update of";
    /// The bound profile.
    pub const PROFILE: &str = "Profile";
    /// The model.
    pub const MODEL: &str = "Model";
    /// The sealed tools.
    pub const TOOLS: &str = "Tools";
    /// The peers, each with its edge's basis.
    pub const PEERS: &str = "Peers";
    /// Peers an update adds.
    pub const PEERS_ADDED: &str = "Peers added";
    /// Peers an update removes.
    pub const PEERS_REMOVED: &str = "Peers removed";
    /// Authority fields.
    pub const AUTHORITY_FIELDS: &str = "Authority fields";
    /// Descriptive labels.
    pub const DESCRIPTIVE_LABELS: &str = "Descriptive labels";
    /// The consequences the policy states.
    pub const CONSEQUENCES: &str = "Consequences";
    /// The host's consequence lines.
    pub const HOST_CONSEQUENCES: &str = "Host consequences";
    /// The heading of an update's free text.
    pub const TEXT_OF: &str = "Text of";
    /// The heading of a created identity's free text, before its ordinal.
    pub const TEXT_OF_IDENTITY: &str = "Text of identity";
    /// The display name.
    pub const DISPLAY_NAME: &str = "Display name";
    /// The purpose.
    pub const PURPOSE: &str = "Purpose";
    /// The creator's instructions.
    pub const INSTRUCTIONS: &str = "Instructions";
    /// The kickoff message.
    pub const KICKOFF: &str = "Kickoff";
    /// The supplied context, as canonical pretty JSON.
    pub const SUPPLIED_CONTEXT: &str = "Supplied context";
    /// The word of an absent value.
    pub const NONE: &str = "none";
    /// The word of an update field that does not change.
    pub const UNCHANGED: &str = "unchanged";
}

/// A `render_v1` rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendering(String);

impl Rendering {
    /// The rendering's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The rendering's text, owned.
    pub fn into_string(self) -> String {
        self.0
    }

    /// The subject digest: the sha256 of the rendering's bytes.
    pub fn subject_digest(&self) -> Sha256Digest {
        Sha256Digest::of(self.0.as_bytes())
    }

    /// The rendering's canonical parts for the part size `max_part_bytes`
    /// (`None`: one part).
    pub fn canonical_parts(&self, max_part_bytes: Option<NonZeroU32>) -> Vec<CanonicalPart> {
        canonical_parts(&self.0, max_part_bytes)
    }

    /// The structured card parsed from the rendering.
    pub fn card(&self) -> Result<StructuredCard, CardError> {
        StructuredCard::parse(&self.0)
    }
}

/// The canonical text of a request, line by line.
#[derive(Default)]
struct Text {
    out: String,
}

impl Text {
    /// End the current block.
    fn block(&mut self) {
        self.out.push('\n');
    }

    fn field(&mut self, label: &str, value: &str) {
        self.out.push_str(label);
        self.out.push_str(": ");
        self.out.push_str(value);
        self.out.push('\n');
    }

    fn list(&mut self, label: &str, items: &[String]) {
        self.out.push_str(label);
        self.out.push_str(" (");
        self.out.push_str(&items.len().to_string());
        self.out.push_str("):\n");
        for item in items {
            self.out.push_str("- ");
            self.out.push_str(item);
            self.out.push('\n');
        }
    }

    fn text(&mut self, label: &str, text: &str) {
        let lines: Vec<&str> = text.split('\n').collect();
        self.out.push_str(label);
        self.out.push_str(" (");
        self.out.push_str(&lines.len().to_string());
        self.out.push_str(if lines.len() == 1 {
            " line):\n"
        } else {
            " lines):\n"
        });
        for line in lines {
            if line.is_empty() {
                self.out.push_str(">\n");
            } else {
                self.out.push_str("> ");
                self.out.push_str(&escaped_line(line));
                self.out.push('\n');
            }
        }
    }

    fn optional_text(&mut self, label: &str, text: Option<&str>, absent: &str) {
        match text {
            Some(text) => self.text(label, text),
            None => self.field(label, absent),
        }
    }
}

/// Render `request` with the host's consequence lines from `hook` (none
/// without a hook). The hook failing refuses the request
/// `consequences_unavailable`: a request is never rendered without its
/// host's lines. Oversize host lines, or an oversize rendering, are refused
/// `text_too_large`.
pub fn render_v1(
    request: &IdentityRequest,
    hook: Option<&dyn ConsequenceHook>,
) -> Result<Rendering, IdentityRefusal> {
    let host_lines = host_consequences(request, hook)?;
    let mut text = Text::default();
    text.field(labels::TITLE, labels::VERSION);
    let action = match request.action {
        RequestAction::Create { .. } => "create",
        RequestAction::Update(_) => "update",
        RequestAction::Remove { .. } => "remove",
    };
    text.field(labels::ACTION, action);
    text.field(labels::REQUESTER, &requester_value(&request.requester));
    text.field(
        labels::PROPOSER,
        &optional_quoted(request.proposer.as_deref()),
    );
    match &request.source_evidence {
        Some(evidence) => {
            text.field(labels::SOURCE_ATTESTOR, &quoted(&evidence.attestor));
            text.field(labels::SOURCE_SUBJECT, &quoted(&evidence.subject));
            text.field(labels::SOURCE_REFERENCE, &quoted(&evidence.source_ref));
            text.field(
                labels::SOURCE_OBSERVED_AT_MS,
                &evidence.observed_at_ms.to_string(),
            );
        }
        None => text.field(labels::SOURCE_EVIDENCE, labels::NONE),
    }
    match &request.action {
        RequestAction::Create { identities } => {
            text.field(labels::IDENTITIES, &identities.len().to_string());
            let count = identities.len();
            for (position, (identity, spec)) in identities.iter().enumerate() {
                text.block();
                created_fields(&mut text, position + 1, count, identity, spec);
            }
            consequence_blocks(&mut text, &request.consequences, &host_lines);
            for (position, (identity, spec)) in identities.iter().enumerate() {
                text.block();
                created_text(&mut text, position + 1, count, identity, spec);
            }
        }
        RequestAction::Update(update) => {
            update_header(&mut text, update);
            text.block();
            update_fields(&mut text, update);
            consequence_blocks(&mut text, &request.consequences, &host_lines);
            text.block();
            update_text(&mut text, update);
        }
        RequestAction::Remove {
            identity,
            expected_revision,
        } => {
            text.field(labels::IDENTITY, &quoted(identity));
            text.field(labels::EXPECTED_REVISION, &expected_revision.to_string());
            consequence_blocks(&mut text, &request.consequences, &host_lines);
        }
    }
    let rendered = text.out;
    if rendered.len() > RENDERING_MAX_BYTES {
        return Err(IdentityRefusal::new(IdentityRefusalKind::TextTooLarge {
            limit: RENDERING_MAX_BYTES as u64,
        })
        .at("rendering"));
    }
    Ok(Rendering(rendered))
}

/// The host's consequence lines for `request`, checked and in byte order.
fn host_consequences(
    request: &IdentityRequest,
    hook: Option<&dyn ConsequenceHook>,
) -> Result<BTreeSet<ConsequenceLine>, IdentityRefusal> {
    let Some(hook) = hook else {
        return Ok(BTreeSet::new());
    };
    let lines = hook.consequences(request).map_err(|refusal| {
        tracing::warn!(%refusal, "the host's identity consequence lines are unavailable");
        IdentityRefusal::new(IdentityRefusalKind::ConsequencesUnavailable)
    })?;
    let mut total = 0_usize;
    let mut checked = BTreeSet::new();
    for (index, line) in lines.into_iter().enumerate() {
        let size = line.text().len();
        if size > HOST_CONSEQUENCE_MAX_BYTES {
            return Err(IdentityRefusal::new(IdentityRefusalKind::TextTooLarge {
                limit: HOST_CONSEQUENCE_MAX_BYTES as u64,
            })
            .at(format!("host_consequences[{index}]")));
        }
        total = total.saturating_add(size);
        if total > HOST_CONSEQUENCES_MAX_BYTES {
            return Err(IdentityRefusal::new(IdentityRefusalKind::TextTooLarge {
                limit: HOST_CONSEQUENCES_MAX_BYTES as u64,
            })
            .at("host_consequences"));
        }
        checked.insert(line);
    }
    Ok(checked)
}

fn requester_value(requester: &Requester) -> String {
    match requester {
        Requester::Agent { identity } => format!("agent {}", quoted(identity)),
        Requester::Host { surface } => format!("host {}", quoted(surface)),
        Requester::Human { principal } => format!("human {}", quoted(principal)),
    }
}

fn optional_quoted(value: Option<&str>) -> String {
    value.map_or_else(|| labels::NONE.to_owned(), quoted)
}

fn quoted_set(values: &BTreeSet<String>) -> Vec<String> {
    values.iter().map(|value| quoted(value)).collect()
}

fn label_items(entries: &BTreeMap<String, String>) -> Vec<String> {
    entries
        .iter()
        .map(|(key, value)| format!("{} = {}", quoted(key), quoted(value)))
        .collect()
}

fn peer_items(peers: &BTreeMap<String, EdgeBasis>) -> Vec<String> {
    peers
        .iter()
        .map(|(peer, basis)| format!("{} basis {}", quoted(peer), basis_value(basis)))
        .collect()
}

fn basis_value(basis: &EdgeBasis) -> String {
    match basis {
        EdgeBasis::Creator => "creator".to_owned(),
        EdgeBasis::CreatorPeer => "creator-peer".to_owned(),
        EdgeBasis::SameBatch => "same-batch".to_owned(),
        EdgeBasis::Rule { id } => format!("rule {}", quoted(id)),
    }
}

fn ordinal_label(prefix: &str, position: usize, count: usize) -> String {
    format!("{prefix} {position} of {count}")
}

fn created_fields(
    text: &mut Text,
    position: usize,
    count: usize,
    identity: &str,
    spec: &CreatedSpec,
) {
    text.field(
        &ordinal_label(labels::IDENTITY, position, count),
        &quoted(identity),
    );
    text.field(labels::TEMPLATE, &quoted(&spec.template));
    text.field(labels::TEMPLATE_BINDING, spec.binding_digest.as_str());
    text.field(labels::PROFILE, &quoted(&spec.profile));
    text.field(labels::MODEL, &quoted(&spec.model));
    text.list(labels::TOOLS, &quoted_set(&spec.tools));
    text.list(labels::PEERS, &peer_items(&spec.peers));
    text.list(
        labels::AUTHORITY_FIELDS,
        &label_items(&spec.authority_fields),
    );
    text.list(
        labels::DESCRIPTIVE_LABELS,
        &label_items(&spec.descriptive_labels),
    );
}

fn created_text(
    text: &mut Text,
    position: usize,
    count: usize,
    identity: &str,
    spec: &CreatedSpec,
) {
    text.field(
        &ordinal_label(labels::TEXT_OF_IDENTITY, position, count),
        &quoted(identity),
    );
    text.text(labels::DISPLAY_NAME, &spec.display_name);
    text.text(labels::PURPOSE, &spec.purpose);
    text.text(labels::INSTRUCTIONS, &spec.instructions);
    text.optional_text(labels::KICKOFF, spec.kickoff.as_deref(), labels::NONE);
    let context = spec.supplied_context.as_ref().map(context_text);
    text.optional_text(labels::SUPPLIED_CONTEXT, context.as_deref(), labels::NONE);
}

fn update_header(text: &mut Text, update: &UpdatedSpec) {
    text.field(labels::IDENTITY, &quoted(&update.identity));
    text.field(
        labels::EXPECTED_REVISION,
        &update.expected_revision.to_string(),
    );
    text.field(labels::TEMPLATE, &quoted(&update.template));
    text.field(labels::TEMPLATE_BINDING, update.binding_digest.as_str());
    text.field(
        labels::TEMPLATE_REBIND,
        update
            .rebind
            .as_ref()
            .map_or(labels::NONE, Sha256Digest::as_str),
    );
    let changes: Vec<&str> = update
        .change_classes()
        .into_iter()
        .map(ChangeClass::token)
        .collect();
    let changes = if changes.is_empty() {
        labels::NONE.to_owned()
    } else {
        changes.join(" ")
    };
    text.field(labels::CHANGES, &changes);
}

fn update_fields(text: &mut Text, update: &UpdatedSpec) {
    text.field(labels::UPDATE_OF, &quoted(&update.identity));
    match &update.tools {
        Some(tools) => text.list(labels::TOOLS, &quoted_set(tools)),
        None => text.field(labels::TOOLS, labels::UNCHANGED),
    }
    text.list(labels::PEERS_ADDED, &peer_items(&update.peers_add));
    text.list(labels::PEERS_REMOVED, &quoted_set(&update.peers_remove));
    match &update.authority_fields {
        Some(fields) => text.list(labels::AUTHORITY_FIELDS, &label_items(fields)),
        None => text.field(labels::AUTHORITY_FIELDS, labels::UNCHANGED),
    }
    match &update.descriptive_labels {
        Some(descriptive) => text.list(labels::DESCRIPTIVE_LABELS, &label_items(descriptive)),
        None => text.field(labels::DESCRIPTIVE_LABELS, labels::UNCHANGED),
    }
}

fn update_text(text: &mut Text, update: &UpdatedSpec) {
    text.field(labels::TEXT_OF, &quoted(&update.identity));
    text.optional_text(
        labels::DISPLAY_NAME,
        update.display_name.as_deref(),
        labels::UNCHANGED,
    );
    text.optional_text(
        labels::PURPOSE,
        update.purpose.as_deref(),
        labels::UNCHANGED,
    );
    text.optional_text(
        labels::INSTRUCTIONS,
        update.instructions.as_deref(),
        labels::UNCHANGED,
    );
    let context = update.supplied_context.as_ref().map(context_text);
    text.optional_text(
        labels::SUPPLIED_CONTEXT,
        context.as_deref(),
        labels::UNCHANGED,
    );
}

fn context_text(context: &BTreeMap<String, Value>) -> String {
    let value = Value::Object(
        context
            .iter()
            .map(|(key, item)| (key.clone(), item.clone()))
            .collect(),
    );
    canonical_json_pretty(&value)
}

fn consequence_blocks(
    text: &mut Text,
    consequences: &BTreeSet<Consequence>,
    host_lines: &BTreeSet<ConsequenceLine>,
) {
    text.block();
    let items: Vec<String> = consequences.iter().map(consequence_item).collect();
    text.list(labels::CONSEQUENCES, &items);
    text.block();
    let host_items: Vec<String> = host_lines.iter().map(|line| quoted(line.text())).collect();
    text.list(labels::HOST_CONSEQUENCES, &host_items);
}

fn consequence_item(consequence: &Consequence) -> String {
    let identity = quoted(&consequence.identity);
    match &consequence.effect {
        ConsequenceEffect::ToolOutsideTemplate { tool } => format!(
            "{identity} tool {} is outside the template ceiling",
            quoted(tool)
        ),
        ConsequenceEffect::ToolNotHeldByCreator { tool } => format!(
            "{identity} tool {} is not held by the creator",
            quoted(tool)
        ),
        ConsequenceEffect::PeerNeedsApproval { peer, rule, reason } => {
            let rule = rule.as_deref().map_or_else(
                || "the default rule".to_owned(),
                |rule| format!("rule {}", quoted(rule)),
            );
            let mut item = format!(
                "{identity} peer {} needs approval under {rule}",
                quoted(peer)
            );
            if let Some(reason) = reason {
                item.push_str(" because ");
                item.push_str(&quoted(reason));
            }
            item
        }
        ConsequenceEffect::AuthorityField { key, value } => format!(
            "{identity} authority field {} = {} becomes authority",
            quoted(key),
            quoted(value)
        ),
        ConsequenceEffect::AnyProfile { profile } => format!(
            "{identity} profile {} is chosen from every mob profile",
            quoted(profile)
        ),
    }
}
