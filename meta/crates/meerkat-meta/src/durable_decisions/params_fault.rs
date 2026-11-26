//! What is wrong with a decision request's params, said without the
//! request's values.
//!
//! `serde_json` decides whether the params of a decision verb are read
//! (`parse` in `stdio.rs`). Its own message for a refusal names no field and
//! quotes the value it found. When it refuses, [`message`] reads the same
//! params once more through [`Read`], a deserializer over the borrowed value
//! whose error is [`Fault`]. serde reports every fault through one of its
//! typed constructors, and [`Fault`] keeps of each only the place, the kind
//! of what was found and what the reading type expected: never the value
//! found, the name of an unknown field or variant, or a message of the
//! reading type's own.
//!
//! The place is the request's own keys and indexes down to the value at
//! fault. The params of the decision verbs are structs all the way down, and
//! a struct reads a value only under a key it knows, so the place names
//! fields. Where serde buffers an object before it reads it (the body of an
//! internally tagged enum), a fault inside is placed at that object.
//!
//! The second read decides nothing: which requests are refused stays
//! `serde_json`'s. It reads a value as `serde_json` reads it, so that it
//! finds the fault `serde_json` found, except for a type the decision verbs
//! do not use: a `serde_json::value::RawValue` is not read.

use std::fmt;

use serde::de::value::{BorrowedStrDeserializer, MapAccessDeserializer};
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, Deserializer, Expected, MapAccess, SeqAccess,
    Unexpected, Visitor,
};
use serde::forward_to_deserialize_any;
use serde_json::Value;

/// The invalid params message for `params`, which did not read as `T`.
pub(super) fn message<T: DeserializeOwned>(params: &Value) -> String {
    match T::deserialize(Read {
        value: params,
        path: "",
    }) {
        Err(fault) => format!("invalid params: {fault}"),
        // The second read found nothing to name: say only what was refused.
        Ok(_) => "invalid params".to_owned(),
    }
}

/// One fault of a request's params: where it is and what is wrong there, in
/// words that carry nothing of the request but its shape.
#[derive(Debug)]
pub(super) struct Fault {
    /// The path of the value at fault, empty for the params themselves.
    /// Unset until the level that handed the value out places the fault.
    path: Option<String>,
    /// The field of that value the fault is about: one it lacks or repeats.
    field: Option<&'static str>,
    what: String,
}

impl Fault {
    fn new(what: String) -> Self {
        Self {
            path: None,
            field: None,
            what,
        }
    }

    fn of_field(field: &'static str, what: &str) -> Self {
        Self {
            path: None,
            field: Some(field),
            what: what.to_owned(),
        }
    }

    /// This fault, at `path` unless a deeper level placed it already.
    fn at(mut self, path: &str) -> Self {
        self.path.get_or_insert_with(|| path.to_owned());
        self
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = &self.what;
        match (self.path.as_deref().unwrap_or_default(), self.field) {
            ("", None) => formatter.write_str(what),
            ("", Some(field)) => write!(formatter, "{field}: {what}"),
            (path, None) => write!(formatter, "{path}: {what}"),
            (path, Some(field)) => write!(formatter, "{path}.{field}: {what}"),
        }
    }
}

impl std::error::Error for Fault {}

impl de::Error for Fault {
    /// A message of the reading type's own may quote the value it refused,
    /// so none of it is kept.
    fn custom<T: fmt::Display>(_message: T) -> Self {
        Self::new("invalid value".to_owned())
    }

    fn invalid_type(found: Unexpected<'_>, expected: &dyn Expected) -> Self {
        Self::new(format!(
            "invalid type: {}, expected {expected}",
            kind(found)
        ))
    }

    fn invalid_value(found: Unexpected<'_>, expected: &dyn Expected) -> Self {
        Self::new(format!(
            "invalid value: {}, expected {expected}",
            kind(found)
        ))
    }

    fn invalid_length(length: usize, expected: &dyn Expected) -> Self {
        Self::new(format!("invalid length {length}, expected {expected}"))
    }

    fn unknown_variant(_variant: &str, expected: &'static [&'static str]) -> Self {
        Self::new(format!("unknown variant, {}", one_of(expected, "variants")))
    }

    fn unknown_field(_field: &str, expected: &'static [&'static str]) -> Self {
        Self::new(format!("unknown field, {}", one_of(expected, "fields")))
    }

    fn missing_field(field: &'static str) -> Self {
        Self::of_field(field, "missing field")
    }

    fn duplicate_field(field: &'static str) -> Self {
        Self::of_field(field, "duplicate field")
    }
}

/// The kind of what was found, in serde's words without the value.
fn kind(found: Unexpected<'_>) -> &'static str {
    match found {
        Unexpected::Bool(_) => "boolean",
        Unexpected::Unsigned(_) | Unexpected::Signed(_) => "integer",
        Unexpected::Float(_) => "floating point",
        Unexpected::Char(_) => "character",
        Unexpected::Str(_) => "string",
        Unexpected::Bytes(_) => "byte array",
        Unexpected::Unit => "null",
        Unexpected::Option => "Option value",
        Unexpected::NewtypeStruct => "newtype struct",
        Unexpected::Seq => "sequence",
        Unexpected::Map => "map",
        Unexpected::Enum => "enum",
        Unexpected::UnitVariant => "unit variant",
        Unexpected::NewtypeVariant => "newtype variant",
        Unexpected::TupleVariant => "tuple variant",
        Unexpected::StructVariant => "struct variant",
        Unexpected::Other(_) => "value",
    }
}

/// The names a reading type accepts, in serde's words.
fn one_of(names: &'static [&'static str], none: &str) -> String {
    match names {
        [] => format!("there are no {none}"),
        [only] => format!("expected `{only}`"),
        [first, second] => format!("expected `{first}` or `{second}`"),
        names => format!("expected one of `{}`", names.join("`, `")),
    }
}

/// One value of the params, read where it lies, and its path from them.
struct Read<'p, 'de> {
    value: &'de Value,
    path: &'p str,
}

impl<'p, 'de> Read<'p, 'de> {
    fn entries(&self, entries: &'de serde_json::Map<String, Value>) -> Entries<'p, 'de> {
        Entries {
            path: self.path,
            entries: entries.iter(),
            pending: None,
        }
    }
}

impl<'de> Deserializer<'de> for Read<'_, 'de> {
    type Error = Fault;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Fault> {
        match self.value {
            Value::Null => visitor.visit_unit(),
            Value::Bool(value) => visitor.visit_bool(*value),
            Value::Number(number) => match (number.as_u64(), number.as_i64(), number.as_f64()) {
                (Some(value), _, _) => visitor.visit_u64(value),
                (None, Some(value), _) => visitor.visit_i64(value),
                (None, None, Some(value)) => visitor.visit_f64(value),
                (None, None, None) => Err(de::Error::custom("a number of no kind")),
            },
            Value::String(value) => visitor.visit_borrowed_str(value),
            // An array or an object the visitor left elements of is refused
            // in `serde_json`'s words for it.
            Value::Array(items) => {
                let mut rest = Items {
                    path: self.path,
                    items: items.iter().enumerate(),
                };
                let read = visitor.visit_seq(&mut rest)?;
                if rest.items.len() == 0 {
                    Ok(read)
                } else {
                    Err(de::Error::invalid_length(
                        items.len(),
                        &"fewer elements in array",
                    ))
                }
            }
            Value::Object(entries) => {
                let mut rest = self.entries(entries);
                let read = visitor.visit_map(&mut rest)?;
                if rest.entries.len() == 0 {
                    Ok(read)
                } else {
                    Err(de::Error::invalid_length(
                        entries.len(),
                        &"fewer elements in map",
                    ))
                }
            }
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Fault> {
        match self.value {
            Value::Null => visitor.visit_none(),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Fault> {
        visitor.visit_newtype_struct(self)
    }

    /// A variant is its name, or an object of the one entry that names it.
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Fault> {
        match self.value {
            Value::String(variant) => visitor.visit_enum(BorrowedStrDeserializer::new(variant)),
            Value::Object(entries) if entries.len() == 1 => {
                visitor.visit_enum(MapAccessDeserializer::new(self.entries(entries)))
            }
            _ => self.deserialize_any(visitor),
        }
    }

    /// A field or a variant is named by a string. serde's own identifier
    /// visitors would take an index too, which `serde_json` refuses.
    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Fault> {
        match self.value {
            Value::String(name) => visitor.visit_borrowed_str(name),
            other => {
                let expected: &dyn Expected = &visitor;
                Err(Fault::new(format!(
                    "invalid type: {}, expected {expected}",
                    shape(other)
                )))
            }
        }
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct
        ignored_any
    }
}

/// The kind of a value of the params, in the words of [`kind`].
fn shape(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_f64() => "floating point",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) => "sequence",
        Value::Object(_) => "map",
    }
}

/// The entries of one object of the params. A fault in a value is placed at
/// its key's path; a fault in a key is left to the object's own place.
struct Entries<'p, 'de> {
    path: &'p str,
    entries: serde_json::map::Iter<'de>,
    /// The entry whose key was read last, until its value is read.
    pending: Option<(&'de str, &'de Value)>,
}

impl<'de> MapAccess<'de> for Entries<'_, 'de> {
    type Error = Fault;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Fault> {
        let Some((key, value)) = self.entries.next() else {
            return Ok(None);
        };
        self.pending = Some((key.as_str(), value));
        seed.deserialize(BorrowedStrDeserializer::new(key.as_str()))
            .map(Some)
    }

    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, seed: S) -> Result<S::Value, Fault> {
        let Some((key, value)) = self.pending.take() else {
            return Err(de::Error::custom("a value is read after its key"));
        };
        let path = if self.path.is_empty() {
            key.to_owned()
        } else {
            format!("{}.{key}", self.path)
        };
        seed.deserialize(Read { value, path: &path })
            .map_err(|fault| fault.at(&path))
    }
}

/// The elements of one array of the params, each placed at its index.
struct Items<'p, 'de> {
    path: &'p str,
    items: std::iter::Enumerate<std::slice::Iter<'de, Value>>,
}

impl<'de> SeqAccess<'de> for Items<'_, 'de> {
    type Error = Fault;

    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        seed: S,
    ) -> Result<Option<S::Value>, Fault> {
        let Some((index, value)) = self.items.next() else {
            return Ok(None);
        };
        let path = format!("{}[{index}]", self.path);
        seed.deserialize(Read { value, path: &path })
            .map(Some)
            .map_err(|fault| fault.at(&path))
    }
}
