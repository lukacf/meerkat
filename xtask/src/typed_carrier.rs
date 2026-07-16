//! Typed-carrier-adoption AST gate.
//!
//! This is an ADD-ONLY ratchet gate. It asserts that a curated set of struct /
//! enum-variant fields hold their canonical TYPED owner type at HEAD, so a
//! future edit that reverts a field back to `String`, `serde_json::Value`, or a
//! loosely-typed shape fails CI.
//!
//! Determinism: `syn::ItemStruct`/`syn::ItemEnum` give each field's *declared*
//! type via `field.ty` (a `syn::Type`). The gate compares the whitespace-
//! normalized token string of that declared type against the registered
//! expected owner type. No type resolution / rust-analyzer is required — the
//! declared type is right there in the AST.
//!
//! Matching is `contains`: the expected owner token must appear inside the
//! normalized declared type. This tolerates legitimate generic wrappers
//! (`Option<..>`, `TurnMetadataOverride<..>`) while still failing on a revert to
//! `String` / `serde_json::Value`, which do not contain the owner token.
//!
//! The registry is keyed on the *type ident* (struct or enum name) plus the
//! field name. It deliberately does NOT key on file path, so a carrier whose
//! canonical type name appears in more than one crate (e.g. a core domain type
//! and its wire projection sharing a name) is locked on every declaration.

use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{ItemEnum, ItemStruct, Type};

use crate::rmat_audit::{Finding, error_finding};

/// One registered carrier: `(type_name, field_name, expected_owner_type)`.
///
/// `type_name` is the struct or enum ident. `field_name` is the named field —
/// for enum-variant fields it is the field name inside the variant. The gate
/// fires when the type declares this field with a type whose normalized token
/// string does NOT contain `expected_owner_type`, or when the field is absent
/// entirely (carrier reverted / removed).
pub type CarrierEntry = (&'static str, &'static str, &'static str);

/// Canonical typed carriers locked at HEAD.
///
/// Every entry here was verified by reading its source declaration: each field
/// already holds its typed owner at HEAD, so this gate is GREEN on current code.
///
/// PENDING carriers (still `Option<serde_json::Value>` / intentionally
/// `String`) are intentionally NOT registered — see the module-level audit notes
/// and the dropped-candidate documentation in the audit report. Registering a
/// PENDING field would make the gate RED on green code, which is forbidden.
pub const TYPED_CARRIER_REGISTRY: &[CarrierEntry] = &[
    // `RuntimeTurnMetadata` is the canonical per-turn runtime metadata seam.
    // Its docstring asserts "`serde_json::Value` does not appear anywhere in
    // this shape" — these locks enforce exactly that invariant per field.
    // meerkat-core/src/lifecycle/run_primitive.rs
    ("RuntimeTurnMetadata", "model", "ModelId"),
    ("RuntimeTurnMetadata", "provider", "Provider"),
    ("RuntimeTurnMetadata", "self_hosted_server_id", "String"),
    (
        "RuntimeTurnMetadata",
        "provider_params",
        "ProviderParamsOverride",
    ),
    ("RuntimeTurnMetadata", "auth_binding", "AuthBindingRef"),
    (
        "RuntimeTurnMetadata",
        "execution_kind",
        "RuntimeExecutionKind",
    ),
    (
        "RuntimeTurnMetadata",
        "peer_response_terminal_apply_intent",
        "PeerResponseTerminalApplyIntent",
    ),
    // `CommsCommandRequest::PeerRequest.intent` is the closed, core-owned
    // request-intent vocabulary. The same-named enum is declared in both
    // meerkat-core (domain) and meerkat-contracts (wire projection); both carry
    // the typed `CommsPeerRequestIntent` at HEAD, so this entry locks the field
    // on every declaration (keyed by type ident, not path).
    // meerkat-core/src/comms.rs + meerkat-contracts/src/wire/comms.rs
    ("CommsCommandRequest", "intent", "CommsPeerRequestIntent"),
    // BY-DESIGN opaque — NOT enforced and MUST NOT be migrated. These three
    // carry arbitrary provider-native knobs and are a deliberate §3 opaque
    // pass-through at the durable/config boundary: `provider_params` only
    // becomes interpretable once provider context exists (projected to a typed
    // `ProviderTag` at build time in meerkat/src/factory.rs). Typing them at
    // this seam breaks the intentional byte-for-byte session-storage contract
    // in meerkat-session/tests/persistence_compat.rs (fixture #4 "Anthropic
    // thinking canary", fixture #5 scalar round-trip). `ProviderParamsOverride`
    // is NOT serde(transparent) and has no merge owner, so it cannot replace the
    // opaque Value here. Confirmed by keystone-carrier (2026-06-09); see the
    // post-execution recalibration in the PR759 re-verification report. The
    // codex ledger and the 531-agent verification both over-flagged these as
    // active STRING_JSON; they are OVERFLAGGED_BY_DESIGN.
    //   AgentConfig.provider_params         => Option<serde_json::Value>  (meerkat-core/src/config.rs)
    //   SessionBuildOptions.provider_params => Option<serde_json::Value>  (meerkat-core/src/service/mod.rs)
    //   SessionMetadata.provider_params     => Option<serde_json::Value>  (meerkat-core/src/session.rs)
    // PENDING (intentionally `String` on the wire seam — wire types do not carry
    // the strict `Provider` enum, so these are NOT carrier reversions):
    //   LoginStartParams.provider     => String  (meerkat-contracts/src/wire/connection.rs)
    //   WireBackendProfile.provider   => String  (meerkat-contracts/src/wire/connection.rs)
    //   WireAuthProfile.provider      => String  (meerkat-contracts/src/wire/connection.rs)
];

/// Normalize a declared type to a stable, whitespace-free token string.
fn normalize_type(ty: &Type) -> String {
    use quote::ToTokens;
    let raw = ty.to_token_stream().to_string();
    raw.chars().filter(|c| !c.is_whitespace()).collect()
}

/// AST visitor that locks registered carrier fields to their typed owner type.
pub struct TypedCarrierAdoptionVisitor<'a> {
    relative: &'a str,
    source: &'a str,
    registry: &'a [CarrierEntry],
    pub findings: Vec<Finding>,
}

impl<'a> TypedCarrierAdoptionVisitor<'a> {
    pub fn new(relative: &'a str, source: &'a str) -> Self {
        Self::with_registry(relative, source, TYPED_CARRIER_REGISTRY)
    }

    pub fn with_registry(relative: &'a str, source: &'a str, registry: &'a [CarrierEntry]) -> Self {
        Self {
            relative,
            source,
            registry,
            findings: Vec::new(),
        }
    }

    /// Check every registered carrier for `type_name` against a flattened view
    /// of all declared `(field_name, normalized_type)` pairs for that type.
    ///
    /// For a struct this is its own fields. For an enum it is the union of every
    /// variant's fields — a carrier declared in any single variant counts as
    /// present, so a sibling variant lacking the field does not produce a false
    /// "missing" finding. A matching field whose declared type does not contain
    /// the expected owner token is flagged; a registered field present in no
    /// variant/field is flagged as "carrier not adopted".
    fn check_named_owner<'f>(
        &mut self,
        type_name: &str,
        fields: impl Iterator<Item = &'f syn::Field>,
        span: proc_macro2::Span,
    ) {
        let declared: Vec<(String, String)> = fields
            .filter_map(|field| {
                field
                    .ident
                    .as_ref()
                    .map(|ident| (ident.to_string(), normalize_type(&field.ty)))
            })
            .collect();

        for (reg_type, reg_field, expected) in self.registry {
            if *reg_type != type_name {
                continue;
            }
            match declared
                .iter()
                .find(|(name, _)| name == reg_field)
                .map(|(_, ty)| ty)
            {
                Some(normalized) => {
                    let expected_normalized: String =
                        expected.chars().filter(|c| !c.is_whitespace()).collect();
                    if !normalized.contains(&expected_normalized) {
                        self.push_reverted(type_name, reg_field, expected, normalized, span);
                    }
                }
                None => self.push_missing(type_name, reg_field, expected, span),
            }
        }
    }

    fn push_reverted(
        &mut self,
        type_name: &str,
        field: &str,
        expected: &str,
        declared: &str,
        span: proc_macro2::Span,
    ) {
        let suppressed =
            crate::rmat_audit::rmat_allow_at(self.source, span, "TypedCarrierAdoption");
        self.findings.push(error_finding(
            "TypedCarrierAdoption",
            self.relative,
            &format!("{type_name}::{field}"),
            format!(
                "typed carrier reverted: field `{type_name}::{field}` declares `{declared}` but must carry its canonical typed owner `{expected}` (a String/serde_json::Value/loosely-typed shape is forbidden)"
            ),
            suppressed,
        ));
    }

    fn push_missing(
        &mut self,
        type_name: &str,
        field: &str,
        expected: &str,
        span: proc_macro2::Span,
    ) {
        let suppressed =
            crate::rmat_audit::rmat_allow_at(self.source, span, "TypedCarrierAdoption");
        self.findings.push(error_finding(
            "TypedCarrierAdoption",
            self.relative,
            &format!("{type_name}::{field}"),
            format!(
                "typed carrier not adopted: registered field `{type_name}::{field}` is missing; it must carry its canonical typed owner `{expected}`"
            ),
            suppressed,
        ));
    }
}

impl<'ast> Visit<'ast> for TypedCarrierAdoptionVisitor<'_> {
    fn visit_item_struct(&mut self, node: &'ast ItemStruct) {
        let type_name = node.ident.to_string();
        self.check_named_owner(&type_name, node.fields.iter(), node.span());
        visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast ItemEnum) {
        let type_name = node.ident.to_string();
        // Enum-variant carriers: the registry keys on the enum ident, and the
        // field lives inside one of the variants. Aggregate all variant fields
        // across every variant so a carrier declared in any variant is observed
        // and a sibling variant lacking the field does not falsely report it
        // "missing".
        let all_variant_fields = node
            .variants
            .iter()
            .flat_map(|variant| variant.fields.iter());
        self.check_named_owner(&type_name, all_variant_fields, node.span());
        visit::visit_item_enum(self, node);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic)]

    use super::*;

    const TEST_REGISTRY: &[CarrierEntry] = &[
        ("RuntimeTurnMetadata", "provider", "Provider"),
        (
            "RuntimeTurnMetadata",
            "provider_params",
            "ProviderParamsOverride",
        ),
        ("CommsCommandRequest", "intent", "CommsPeerRequestIntent"),
    ];

    fn findings(source: &str) -> Vec<Finding> {
        let parsed = syn::parse_file(source).expect("parse typed-carrier fixture");
        let mut visitor = TypedCarrierAdoptionVisitor::with_registry(
            "meerkat-core/src/x.rs",
            source,
            TEST_REGISTRY,
        );
        visitor.visit_file(&parsed);
        visitor.findings
    }

    #[test]
    fn accepts_typed_struct_carriers_with_generic_wrappers() {
        let clean = findings(
            r"
            pub struct RuntimeTurnMetadata {
                pub provider: Option<Provider>,
                pub provider_params: Option<TurnMetadataOverride<ProviderParamsOverride>>,
            }
        ",
        );
        assert!(clean.is_empty(), "typed carriers must not flag: {clean:#?}");
    }

    #[test]
    fn accepts_typed_enum_variant_carrier() {
        let clean = findings(
            r"
            pub enum CommsCommandRequest {
                Input { body: String },
                PeerRequest { to: PeerId, intent: CommsPeerRequestIntent },
            }
        ",
        );
        assert!(
            clean.is_empty(),
            "typed enum-variant carrier must not flag: {clean:#?}"
        );
    }

    #[test]
    fn flags_value_reversion_on_struct_field() {
        let flagged = findings(
            r"
            pub struct RuntimeTurnMetadata {
                pub provider: Option<Provider>,
                pub provider_params: Option<serde_json::Value>,
            }
        ",
        );
        assert_eq!(flagged.len(), 1, "exactly the reverted field: {flagged:#?}");
        assert_eq!(flagged[0].key.rule, "TypedCarrierAdoption");
        assert_eq!(
            flagged[0].key.symbol,
            "RuntimeTurnMetadata::provider_params"
        );
        assert!(flagged[0].message.contains("typed carrier reverted"));
    }

    #[test]
    fn flags_string_reversion_on_enum_variant_field() {
        let flagged = findings(
            r"
            pub enum CommsCommandRequest {
                PeerRequest { to: PeerId, intent: String },
            }
        ",
        );
        assert_eq!(flagged.len(), 1, "reverted variant field: {flagged:#?}");
        assert_eq!(flagged[0].key.symbol, "CommsCommandRequest::intent");
    }

    #[test]
    fn flags_missing_registered_field() {
        let flagged = findings(
            r"
            pub struct RuntimeTurnMetadata {
                pub provider: Option<Provider>,
            }
        ",
        );
        assert_eq!(flagged.len(), 1, "missing carrier: {flagged:#?}");
        assert_eq!(
            flagged[0].key.symbol,
            "RuntimeTurnMetadata::provider_params"
        );
        assert!(flagged[0].message.contains("not adopted"));
    }

    #[test]
    fn ignores_unrelated_types() {
        let clean = findings(
            r"
            pub struct SomethingElse {
                pub provider_params: Option<serde_json::Value>,
            }
        ",
        );
        assert!(clean.is_empty(), "unrelated type must not flag: {clean:#?}");
    }
}
