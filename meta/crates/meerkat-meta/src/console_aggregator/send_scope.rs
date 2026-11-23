//! Who a console send's dedupe record belongs to.
//!
//! A console send is deduplicated on its idempotency key inside the scope of
//! the caller that sent it: a verified Console human (OIDC issuer and
//! subject), an authenticated principal with no verified human behind it,
//! or the shared local account of an anonymous caller. The same key under
//! two scopes names two different records, so a replay answers only the
//! caller whose send it was.
//!
//! A send is to a member: an identity of one of the host's mobs. Two mobs
//! may each have a member of the same name, so the key of a send to a member
//! of a mob other than the host's default mob carries the mob too
//! ([`ConsoleSendScope::member_dedupe_key`]). One runtime key names one
//! runtime, however many mobs the runtime hosts.

use meerkat_mob::MobId;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::member_key::DefaultMob;

/// Versioned domain of the scoped dedupe digest.
const SCOPED_SEND_KEY_DOMAIN: &str = "meta-console-send-v1";

/// Prefix of a scoped send dedupe key. Unscoped keys keep the `send:` shape.
const SCOPED_SEND_KEY_PREFIX: &str = "send-scoped:";

/// Versioned domain of the dedupe digest of a send to a member of a mob
/// other than the host's default mob. No send to the default mob has it.
const MEMBER_SEND_KEY_DOMAIN: &str = "meta-console-member-send-v1";

/// Prefix of an unscoped send's dedupe key when the send is to a member of
/// a mob other than the host's default mob: a digest, never the `send:`
/// shape, whose parts a mob could not be told apart in.
const MEMBER_SEND_KEY_PREFIX: &str = "send-member:";

/// The caller a console send is deduplicated for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum ConsoleSendScope {
    /// No Console caller: the public Rust send API. Its keys keep the shape
    /// those callers have always had.
    Unscoped,
    /// An anonymous caller of an open Console: the shared local account.
    #[default]
    Anonymous,
    /// A verified Console human, by OIDC issuer and subject.
    Human { issuer: String, subject: String },
    /// An authenticated principal with no verified human behind it: a
    /// service identity, or a token without a subject. Scoped by its trust
    /// source too (the provider its token was classified as, and the issuer
    /// it verified against), so one name from two sources is two callers.
    Principal {
        principal: String,
        provider: String,
        issuer: String,
    },
}

impl ConsoleSendScope {
    /// Whether this is a Console caller's scope. Only scoped sends are
    /// reconciled or recovered; the unscoped API only replays.
    pub(crate) fn is_scoped(&self) -> bool {
        !matches!(self, Self::Unscoped)
    }

    /// The dedupe key of one send in this scope.
    ///
    /// A scoped key is one digest over every component, so it neither
    /// carries the scope in clear nor links two sends of one caller.
    pub(crate) fn dedupe_key(
        &self,
        runtime_key: &str,
        identity: &str,
        origin: &str,
        idempotency_key: &str,
    ) -> String {
        let Some(owner) = self.owner() else {
            return unscoped_send_dedupe_key(runtime_key, identity, origin, idempotency_key);
        };
        let input = json!([
            SCOPED_SEND_KEY_DOMAIN,
            runtime_key,
            identity,
            origin,
            idempotency_key,
            owner
        ]);
        let digest = Sha256::digest(input.to_string().as_bytes());
        format!("{SCOPED_SEND_KEY_PREFIX}{digest:x}")
    }

    /// The dedupe key of one send in this scope to the member `identity` of
    /// the mob `mob_id`, on a host whose default mob is `default_mob`.
    ///
    /// A send to a member of the default mob has the key
    /// [`Self::dedupe_key`] gives it, byte for byte, however its mob is
    /// named: a send recorded before a send named a mob is still found by
    /// its key. A send to a member of another mob has a key of a digest
    /// domain of its own, with the mob as a component of its own: two mobs'
    /// members of one name never share a send record, and the mob is never
    /// read out of the identity or into it. Such a key is a digest in every
    /// scope, the unscoped one included.
    pub(crate) fn member_dedupe_key(
        &self,
        runtime_key: &str,
        default_mob: &DefaultMob,
        mob_id: &MobId,
        identity: &str,
        origin: &str,
        idempotency_key: &str,
    ) -> String {
        // A blank mob names none, as in every stored record.
        let Some(mob_id) = default_mob
            .stored(mob_id)
            .filter(|mob_id| !mob_id.as_str().is_empty())
        else {
            return self.dedupe_key(runtime_key, identity, origin, idempotency_key);
        };
        let (prefix, owner) = match self.owner() {
            Some(owner) => (SCOPED_SEND_KEY_PREFIX, owner),
            None => (MEMBER_SEND_KEY_PREFIX, json!(["unscoped"])),
        };
        let input = json!([
            MEMBER_SEND_KEY_DOMAIN,
            runtime_key,
            mob_id.as_str(),
            identity,
            origin,
            idempotency_key,
            owner
        ]);
        let digest = Sha256::digest(input.to_string().as_bytes());
        format!("{prefix}{digest:x}")
    }

    /// The caller of a scoped send, as a component of its key's digest;
    /// none for the unscoped API.
    fn owner(&self) -> Option<serde_json::Value> {
        match self {
            Self::Unscoped => None,
            Self::Anonymous => Some(json!(["anonymous"])),
            Self::Human { issuer, subject } => Some(json!(["human", issuer, subject])),
            Self::Principal {
                principal,
                provider,
                issuer,
            } => Some(json!(["principal", provider, issuer, principal])),
        }
    }
}

/// The key shape of an unscoped send, and of every send recorded before
/// sends were scoped.
pub(crate) fn unscoped_send_dedupe_key(
    runtime_key: &str,
    identity: &str,
    origin: &str,
    idempotency_key: &str,
) -> String {
    format!("send:{runtime_key}:{identity}:{origin}:{idempotency_key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn human(subject: &str) -> ConsoleSendScope {
        ConsoleSendScope::Human {
            issuer: "https://issuer.example".to_string(),
            subject: subject.to_string(),
        }
    }

    fn principal(name: &str, provider: &str, issuer: &str) -> ConsoleSendScope {
        ConsoleSendScope::Principal {
            principal: name.to_string(),
            provider: provider.to_string(),
            issuer: issuer.to_string(),
        }
    }

    #[test]
    fn scoped_keys_are_stable_and_separate_every_component() {
        let key = |scope: &ConsoleSendScope| scope.dedupe_key("runtime", "agent", "console", "k1");
        let alice = human("alice-sub");
        assert_eq!(key(&alice), key(&alice));
        assert!(key(&alice).starts_with("send-scoped:"));
        // Each scope differs from some other in exactly one component (the
        // kind, a subject, a principal name, a provider or an issuer); no
        // two share a key.
        let scopes = [
            alice.clone(),
            human("bob-sub"),
            ConsoleSendScope::Human {
                issuer: "https://other.example".to_string(),
                subject: "alice-sub".to_string(),
            },
            ConsoleSendScope::Anonymous,
            principal("alice-sub", "generic_oidc", "https://issuer.example"),
            principal("bob-sub", "generic_oidc", "https://issuer.example"),
            principal("alice-sub", "google_oauth", "https://issuer.example"),
            principal("alice-sub", "service_identity", "https://issuer.example"),
            principal("alice-sub", "generic_oidc", "https://other.example"),
        ];
        for (index, scope) in scopes.iter().enumerate() {
            for other in &scopes[index + 1..] {
                assert_ne!(key(scope), key(other), "{scope:?} and {other:?}");
            }
        }
        for (runtime, identity, origin, idempotency) in [
            ("runtime-b", "agent", "console", "k1"),
            ("runtime", "agent-b", "console", "k1"),
            ("runtime", "agent", "api", "k1"),
            ("runtime", "agent", "console", "k2"),
        ] {
            assert_ne!(
                key(&alice),
                alice.dedupe_key(runtime, identity, origin, idempotency)
            );
        }
    }

    #[test]
    fn scoped_keys_do_not_carry_the_scope_or_the_send_in_clear() {
        let key = human("alice-sub").dedupe_key("runtime", "agent", "console", "k1");
        for clear in ["alice-sub", "issuer.example", "agent", "console", "k1"] {
            assert!(!key.contains(clear), "{key} carries {clear}");
        }
    }

    #[test]
    fn component_boundaries_cannot_be_shifted() {
        let scope = ConsoleSendScope::Anonymous;
        assert_ne!(
            scope.dedupe_key("a:b", "c", "o", "k"),
            scope.dedupe_key("a", "b:c", "o", "k")
        );
        let named = |name: &str| principal(name, "generic_oidc", "https://issuer.example");
        assert_ne!(
            named("x\",\"y").dedupe_key("r", "i", "o", "k"),
            named("x").dedupe_key("r", "i", "o", "\",\"y\"")
        );
    }

    #[test]
    fn unscoped_keys_keep_the_recorded_shape() {
        assert_eq!(
            ConsoleSendScope::Unscoped.dedupe_key("runtime", "agent", "console", "k1"),
            "send:runtime:agent:console:k1"
        );
        assert_ne!(
            ConsoleSendScope::Anonymous.dedupe_key("runtime", "agent", "console", "k1"),
            "send:runtime:agent:console:k1"
        );
    }

    fn member_keyed_scopes() -> [ConsoleSendScope; 4] {
        [
            ConsoleSendScope::Unscoped,
            ConsoleSendScope::Anonymous,
            human("alice-sub"),
            principal("alice-sub", "generic_oidc", "https://issuer.example"),
        ]
    }

    fn member_keyed_default_mob() -> DefaultMob {
        DefaultMob::new(MobId::from("launch"))
    }

    /// A send to a member of the host's default mob keeps the key it always
    /// had, byte for byte, in every scope: a send recorded before a send
    /// named a mob is still found by its key.
    #[test]
    fn member_keyed_send_a_send_to_the_default_mobs_member_keeps_its_key() {
        let default_mob = member_keyed_default_mob();
        let recorded = [
            "send:runtime:agent:console:k1",
            "send-scoped:ac53a6039342e33fc771b6cd288c269e9b4ef1ef029b2f4e9b31ab86550b38b6",
            "send-scoped:b066d55b49fbde40abcf7f003441cd28c01821c1dc3a5a0029b8c027a4516301",
            "send-scoped:911b9b7579dbf3d5c19f0deea4f16741f70093aeba82daf72535a189ff667737",
        ];
        for (scope, recorded) in member_keyed_scopes().iter().zip(recorded) {
            assert_eq!(
                scope.dedupe_key("runtime", "agent", "console", "k1"),
                recorded,
                "{scope:?}"
            );
            assert_eq!(
                scope.member_dedupe_key(
                    "runtime",
                    &default_mob,
                    &MobId::from("launch"),
                    "agent",
                    "console",
                    "k1"
                ),
                recorded,
                "{scope:?}: the default mob named by its id"
            );
            assert_eq!(
                scope.member_dedupe_key(
                    "runtime",
                    &default_mob,
                    &MobId::from(""),
                    "agent",
                    "console",
                    "k1"
                ),
                recorded,
                "{scope:?}: a blank mob names none"
            );
        }
    }

    /// Two mobs of one host each have a member of the same name. A send to
    /// one with an idempotency key already used for the other is its own
    /// send: the two have two keys, in every scope, and so do the members of
    /// that name in two other mobs.
    #[test]
    fn member_keyed_send_two_mobs_members_of_one_name_have_two_keys() {
        let default_mob = member_keyed_default_mob();
        for scope in member_keyed_scopes() {
            let key = |mob: &str| {
                scope.member_dedupe_key(
                    "runtime",
                    &default_mob,
                    &MobId::from(mob),
                    "agent",
                    "console",
                    "k1",
                )
            };
            assert_eq!(key("second"), key("second"), "{scope:?}: a key is stable");
            assert_ne!(key("launch"), key("second"), "{scope:?}");
            assert_ne!(key("second"), key("third"), "{scope:?}");
        }
        // The other components still tell two sends apart.
        let alice = human("alice-sub");
        let second = MobId::from("second");
        let key =
            alice.member_dedupe_key("runtime", &default_mob, &second, "agent", "console", "k1");
        for (runtime, identity, origin, idempotency) in [
            ("runtime-b", "agent", "console", "k1"),
            ("runtime", "agent-b", "console", "k1"),
            ("runtime", "agent", "api", "k1"),
            ("runtime", "agent", "console", "k2"),
        ] {
            assert_ne!(
                key,
                alice.member_dedupe_key(
                    runtime,
                    &default_mob,
                    &second,
                    identity,
                    origin,
                    idempotency
                )
            );
        }
        assert_ne!(
            key,
            human("bob-sub").member_dedupe_key(
                "runtime",
                &default_mob,
                &second,
                "agent",
                "console",
                "k1"
            )
        );
    }

    /// The mob is a component of its own in the key of a send to another
    /// mob's member: it is never read out of the identity or into it, the
    /// key is no key a send to the default mob can have, and it carries
    /// neither the mob nor the send in clear, in every scope.
    #[test]
    fn member_keyed_send_the_mob_is_a_component_of_its_own() {
        let default_mob = member_keyed_default_mob();
        for scope in member_keyed_scopes() {
            let key = |runtime: &str, mob: &str, identity: &str| {
                scope.member_dedupe_key(
                    runtime,
                    &default_mob,
                    &MobId::from(mob),
                    identity,
                    "console",
                    "k1",
                )
            };
            assert_ne!(
                key("runtime", "x", "y:z"),
                key("runtime", "x:y", "z"),
                "{scope:?}"
            );
            assert_ne!(
                key("runtime:x", "y", "z"),
                key("runtime", "x:y", "z"),
                "{scope:?}"
            );
            let second = key("runtime", "second", "agent");
            for default_mob_identity in ["agent", "second:agent", "second/agent"] {
                assert_ne!(
                    second,
                    scope.dedupe_key("runtime", default_mob_identity, "console", "k1"),
                    "{scope:?}: the default mob's {default_mob_identity}"
                );
                assert_ne!(
                    second,
                    scope.dedupe_key("runtime:second", default_mob_identity, "console", "k1"),
                    "{scope:?}: the default mob's {default_mob_identity}"
                );
            }
            assert!(
                !second.starts_with("send:"),
                "{scope:?}: {second} has the shape of a send to the default mob"
            );
            for clear in ["second", "agent", "console", "k1", "alice-sub"] {
                assert!(
                    !second.contains(clear),
                    "{scope:?}: {second} carries {clear}"
                );
            }
        }
    }
}
