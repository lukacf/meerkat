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

impl InputVariantId {
    /// Construct from a crate-owned catalog literal.
    ///
    /// This is intentionally crate-private: schema catalog metadata can use
    /// typed identities without fallible runtime parsing, while external
    /// callers still go through [`Self::parse`].
    pub(crate) fn from_trusted_catalog_literal(value: &'static str) -> Self {
        Self(value.to_owned())
    }
}
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
define_identity!(
    /// Driver name within a composition.
    CompositionDriverId
);
define_identity!(
    /// Transaction plan name within a composition.
    TransactionPlanId
);
define_identity!(
    /// Transaction trigger name within a composition.
    TransactionTriggerId
);
define_identity!(
    /// Witness name within a composition.
    CompositionWitnessId
);
define_identity!(
    /// Entry input name within a composition.
    EntryInputId
);

/// Store primitive referenced by a composition transaction plan.
///
/// Unlike kernel slugs, store primitives name existing Rust-side atomic
/// operations and may use qualified path syntax such as
/// `ScheduleStore::claim_due_occurrences`. The type still owns validation at
/// the schema boundary instead of letting transaction plans carry raw strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StorePrimitiveId(String);

impl StorePrimitiveId {
    pub fn parse(value: impl Into<String>) -> Result<Self, IdentityError> {
        let raw = value.into();
        if raw.is_empty() {
            return Err(IdentityError {
                kind: IdentityErrorKind::Empty,
                raw,
            });
        }
        for (position, ch) in raw.chars().enumerate() {
            if ch.is_control() || ch.is_whitespace() {
                return Err(IdentityError {
                    kind: IdentityErrorKind::InvalidChar { ch, position },
                    raw,
                });
            }
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for StorePrimitiveId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StorePrimitiveId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for StorePrimitiveId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for StorePrimitiveId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(raw).map_err(serde::de::Error::custom)
    }
}

/// Payload field shapes for structural variants in a [`RustTypeAtom::TypePathEnum`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypePathEnumPayloadAtom {
    StringSet,
}

/// One field in a structural enum-variant sample carried by the typed owner.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypePathEnumPayloadField {
    pub name: FieldId,
    pub atom: TypePathEnumPayloadAtom,
}

impl TypePathEnumPayloadField {
    /// Construct a payload field whose value is a finite set of strings.
    pub fn string_set(name: &str) -> Self {
        Self {
            #[allow(clippy::expect_used)]
            name: FieldId::parse(name).expect("valid structural enum field slug"),
            atom: TypePathEnumPayloadAtom::StringSet,
        }
    }
}

/// Structural enum variants whose sample values are represented as tagged maps.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypePathEnumStructuralVariant {
    pub variant: EnumVariantId,
    pub fields: Vec<TypePathEnumPayloadField>,
}

impl TypePathEnumStructuralVariant {
    /// Construct a one-field structural variant with a string-set payload.
    pub fn string_set(variant: &str, field: &str) -> Self {
        Self {
            #[allow(clippy::expect_used)]
            variant: EnumVariantId::parse(variant).expect("valid enum variant slug"),
            fields: vec![TypePathEnumPayloadField::string_set(field)],
        }
    }
}

/// Field value shapes for structural type-path records.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypePathStructFieldAtom {
    String,
    Named(NamedTypeId),
}

/// One field in a structural record carried by the typed owner.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TypePathStructField {
    pub name: FieldId,
    pub atom: TypePathStructFieldAtom,
}

impl TypePathStructField {
    /// Construct a structural field whose value is a string.
    pub fn string(name: &str) -> Self {
        Self {
            #[allow(clippy::expect_used)]
            name: FieldId::parse(name).expect("valid structural record field slug"),
            atom: TypePathStructFieldAtom::String,
        }
    }

    /// Construct a structural field whose value is another named type.
    pub fn named(name: &str, type_name: &str) -> Self {
        #[allow(clippy::expect_used)]
        let type_name = NamedTypeId::parse(type_name).expect("valid nested named-type slug");
        Self {
            #[allow(clippy::expect_used)]
            name: FieldId::parse(name).expect("valid structural record field slug"),
            atom: TypePathStructFieldAtom::Named(type_name),
        }
    }
}

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
    /// String-backed closed semantic domain.
    ///
    /// This keeps the schema-level representation compatible with DSL enum
    /// literals while giving codegen and the runtime oracle an authoritative
    /// finite value set for named string types.
    StringEnum {
        variants: Vec<EnumVariantId>,
    },
    /// Fully-qualified Rust type path, e.g. `"crate::domain::MySpecialType"`.
    TypePath(String),
    /// Fully-qualified Rust struct type path whose model domain is represented
    /// as finite sets of present field names.
    TypePathFieldPresenceSet {
        path: String,
        fields: Vec<FieldId>,
    },
    /// Fully-qualified Rust struct type path whose model and runtime domains are
    /// represented as structural records with typed fields.
    TypePathStruct {
        path: String,
        fields: Vec<TypePathStructField>,
    },
    /// Fully-qualified Rust enum type path with explicit unit variants that
    /// can appear as DSL named-variant literals.
    TypePathEnum {
        path: String,
        unit_variants: Vec<EnumVariantId>,
        #[serde(default)]
        structural_variants: Vec<TypePathEnumStructuralVariant>,
    },
}

impl RustTypeAtom {
    /// Returns whether two named-type bindings project to the same composition
    /// model domain shape.
    ///
    /// Machine-local `TypePath` owners can differ by Rust module path while
    /// still sharing a composition-level TLA domain through the named slug.
    /// `TypePathEnum` owners are likewise path-agnostic here, but their unit
    /// and structural variant payload shapes must agree because those variants
    /// define the generated finite domain.
    pub fn has_same_composition_domain_shape(&self, other: &Self) -> bool {
        if self == other {
            return true;
        }

        match (self, other) {
            (Self::TypePath(_), Self::TypePath(_)) => true,
            (
                Self::TypePathFieldPresenceSet {
                    fields: left_fields,
                    ..
                },
                Self::TypePathFieldPresenceSet {
                    fields: right_fields,
                    ..
                },
            ) => left_fields == right_fields,
            (
                Self::TypePathStruct {
                    fields: left_fields,
                    ..
                },
                Self::TypePathStruct {
                    fields: right_fields,
                    ..
                },
            ) => left_fields == right_fields,
            (
                Self::TypePathEnum {
                    unit_variants: left_units,
                    structural_variants: left_structural,
                    ..
                },
                Self::TypePathEnum {
                    unit_variants: right_units,
                    structural_variants: right_structural,
                    ..
                },
            ) => left_units == right_units && left_structural == right_structural,
            _ => false,
        }
    }
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

    /// Construct a binding whose Rust representation is a closed string
    /// domain rendered as a Rust enum.
    ///
    /// Panics if `name` or any variant is not a valid slug, or if the variant
    /// set is empty. Intended for catalog construction sites.
    pub fn string_enum(name: &str, variants: &[&str]) -> Self {
        assert!(
            !variants.is_empty(),
            "string enum named-type bindings require at least one variant"
        );
        Self {
            #[allow(clippy::expect_used)]
            name: NamedTypeId::parse(name).expect("valid named-type slug"),
            rust: RustTypeAtom::StringEnum {
                variants: variants
                    .iter()
                    .map(|variant| {
                        #[allow(clippy::expect_used)]
                        EnumVariantId::parse(*variant).expect("valid enum variant slug")
                    })
                    .collect(),
            },
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

    /// Construct a binding whose Rust representation is a fully-qualified type
    /// path and whose generated model domain is finite field-presence sets.
    pub fn type_path_field_presence_set(
        name: &str,
        rust_path: impl Into<String>,
        fields: &[&str],
    ) -> Self {
        assert!(
            !fields.is_empty(),
            "field-presence named-type bindings require at least one field"
        );
        Self {
            #[allow(clippy::expect_used)]
            name: NamedTypeId::parse(name).expect("valid named-type slug"),
            rust: RustTypeAtom::TypePathFieldPresenceSet {
                path: rust_path.into(),
                fields: fields
                    .iter()
                    .map(|field| {
                        #[allow(clippy::expect_used)]
                        FieldId::parse(*field).expect("valid field-presence slug")
                    })
                    .collect(),
            },
        }
    }

    /// Construct a binding whose Rust representation is a fully-qualified type
    /// path and whose generated model/runtime domain is a typed structural
    /// record.
    pub fn type_path_struct(
        name: &str,
        rust_path: impl Into<String>,
        fields: Vec<TypePathStructField>,
    ) -> Self {
        assert!(
            !fields.is_empty(),
            "struct named-type bindings require at least one field"
        );
        Self {
            #[allow(clippy::expect_used)]
            name: NamedTypeId::parse(name).expect("valid named-type slug"),
            rust: RustTypeAtom::TypePathStruct {
                path: rust_path.into(),
                fields,
            },
        }
    }

    /// Construct a binding whose Rust representation is a fully-qualified
    /// structural enum type path with a closed variant domain.
    pub fn type_path_enum(
        name: &str,
        rust_path: impl Into<String>,
        unit_variants: &[&str],
    ) -> Self {
        assert!(
            !unit_variants.is_empty(),
            "type-path enum named-type bindings require at least one unit variant"
        );
        Self {
            #[allow(clippy::expect_used)]
            name: NamedTypeId::parse(name).expect("valid named-type slug"),
            rust: RustTypeAtom::TypePathEnum {
                path: rust_path.into(),
                unit_variants: unit_variants
                    .iter()
                    .map(|variant| {
                        #[allow(clippy::expect_used)]
                        EnumVariantId::parse(*variant).expect("valid enum variant slug")
                    })
                    .collect(),
                structural_variants: Vec::new(),
            },
        }
    }

    /// Construct a binding whose Rust representation is a fully-qualified
    /// structural enum type path with unit and payload-carrying variants.
    pub fn type_path_enum_with_structural_variants(
        name: &str,
        rust_path: impl Into<String>,
        unit_variants: &[&str],
        structural_variants: Vec<TypePathEnumStructuralVariant>,
    ) -> Self {
        let mut binding = Self::type_path_enum(name, rust_path, unit_variants);
        if let RustTypeAtom::TypePathEnum {
            structural_variants: variants,
            ..
        } = &mut binding.rust
        {
            *variants = structural_variants;
        }
        binding
    }
}
