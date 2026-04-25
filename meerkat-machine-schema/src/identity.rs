//! Slug-validated identifier newtypes for the machine schema layer.
//!
//! Every identity in the machine kernel vocabulary — machine names, phase names,
//! variant names, field names, transitions, routes, protocols, actors, enum type
//! names and variants, and composition names — is represented here as a distinct
//! newtype wrapping a validated ASCII slug. This closes the first dogma gap in
//! wave (b): kernel identities stop being bare `String` and become typed, so that
//! the compiler rejects field/phase/variant cross-contamination at the boundary
//! instead of the runtime.
//!
//! Validation rules (identical for every identity type):
//! - non-empty
//! - first character: ASCII alphabetic or `_`
//! - subsequent characters: ASCII alphanumeric, `_`, or `-`
//!
//! Anything else — spaces, dots, slashes, control characters, non-ASCII — is
//! rejected at construction time with a structured [`IdentityError`].

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use thiserror::Error;

/// Why an identity string failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityErrorKind {
    /// The input string was empty.
    Empty,
    /// The first character was not ASCII alpha or underscore.
    InvalidStartChar(char),
    /// A later character was not ASCII alphanumeric, underscore, or hyphen.
    InvalidChar { ch: char, position: usize },
}

/// Structured error returned by every identity `parse` constructor.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub struct IdentityError {
    pub kind: IdentityErrorKind,
    pub raw: String,
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            IdentityErrorKind::Empty => {
                write!(f, "identity must not be empty")
            }
            IdentityErrorKind::InvalidStartChar(ch) => {
                write!(
                    f,
                    "identity {:?} must start with ASCII letter or underscore, found {:?}",
                    self.raw, ch
                )
            }
            IdentityErrorKind::InvalidChar { ch, position } => {
                write!(
                    f,
                    "identity {:?} contains invalid character {:?} at position {}",
                    self.raw, ch, position
                )
            }
        }
    }
}

fn validate_slug(raw: &str) -> Result<(), IdentityErrorKind> {
    let mut chars = raw.chars().enumerate();
    let (_, first) = chars.next().ok_or(IdentityErrorKind::Empty)?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(IdentityErrorKind::InvalidStartChar(first));
    }
    for (pos, ch) in chars {
        if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
            return Err(IdentityErrorKind::InvalidChar { ch, position: pos });
        }
    }
    Ok(())
}

macro_rules! define_identity {
    ($(#[$attr:meta])* $name:ident) => {
        $(#[$attr])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            /// Parse a slug, returning a structured error on violation.
            pub fn parse(value: impl Into<String>) -> Result<Self, IdentityError> {
                let raw = value.into();
                match validate_slug(&raw) {
                    Ok(()) => Ok(Self(raw)),
                    Err(kind) => Err(IdentityError { kind, raw }),
                }
            }

            /// Borrow the underlying validated slug.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                Self::parse(raw).map_err(serde::de::Error::custom)
            }
        }
    };
}

define_identity!(
    /// Name of a declared machine (e.g. `"MobMachine"`).
    MachineId
);
define_identity!(
    /// Instance id of a machine within a composition (e.g. `"mob"`).
    MachineInstanceId
);
define_identity!(
    /// Phase name within a machine.
    PhaseId
);
define_identity!(
    /// Input-variant name.
    InputVariantId
);
define_identity!(
    /// Signal-variant name.
    SignalVariantId
);
define_identity!(
    /// Effect-variant name.
    EffectVariantId
);
define_identity!(
    /// Field name within a kernel state, input, signal, or effect.
    FieldId
);
define_identity!(
    /// Transition name.
    TransitionId
);
define_identity!(
    /// Route name within a composition.
    RouteId
);
define_identity!(
    /// Protocol name (e.g. for effect handoff).
    ProtocolId
);
define_identity!(
    /// Actor name within a composition.
    ActorId
);
define_identity!(
    /// Named type alias declared in the DSL.
    NamedTypeId
);
define_identity!(
    /// Enum type declared in the DSL.
    EnumTypeId
);
define_identity!(
    /// Variant name inside an enum type.
    EnumVariantId
);
define_identity!(
    /// Composition name.
    CompositionId
);

/// Atomic Rust-level representation used by [`NamedTypeBinding`] to anchor a
/// DSL-declared named type to the concrete Rust type codegen must emit.
///
/// Grown as needed by wave-b codegen. Intentionally small and explicit — avoids
/// the old `render_named_type_alias_target` allow-list which silently defaulted
/// unknown aliases to `String`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum RustTypeAtom {
    U64,
    U32,
    U16,
    U8,
    Bool,
    String,
    /// Fully-qualified Rust type path, e.g. `"crate::domain::MySpecialType"`.
    TypePath(String),
}

/// Authoritative binding from a DSL-declared named type to its Rust atom.
///
/// Consumed by codegen (B-2) to replace the hard-coded allow-list. The mapping
/// is carried on the DSL declaration itself so the schema layer is the single
/// source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NamedTypeBinding {
    pub name: NamedTypeId,
    pub rust: RustTypeAtom,
}

impl NamedTypeBinding {
    /// Construct a binding whose Rust representation is `u64`.
    ///
    /// Panics if `name` is not a valid [`NamedTypeId`] slug. Intended for
    /// catalog/compat construction sites; callers that want fallible
    /// construction should build [`NamedTypeId`] directly and assemble
    /// the struct by hand.
    pub fn u64(name: &str) -> Self {
        Self {
            #[allow(clippy::expect_used)]
            name: NamedTypeId::parse(name).expect("valid named-type slug"),
            rust: RustTypeAtom::U64,
        }
    }

    /// Construct a binding whose Rust representation is `String`.
    pub fn string(name: &str) -> Self {
        Self {
            #[allow(clippy::expect_used)]
            name: NamedTypeId::parse(name).expect("valid named-type slug"),
            rust: RustTypeAtom::String,
        }
    }

    /// Construct a binding whose Rust representation is a fully-qualified
    /// type path.
    pub fn type_path(name: &str, rust_path: impl Into<String>) -> Self {
        Self {
            #[allow(clippy::expect_used)]
            name: NamedTypeId::parse(name).expect("valid named-type slug"),
            rust: RustTypeAtom::TypePath(rust_path.into()),
        }
    }
}
