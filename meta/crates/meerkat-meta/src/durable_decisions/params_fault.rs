//! What is wrong with a decision request's params, said without anything
//! the request sent.
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
//! The place is a path of steps from the params to the value at fault
//! ([`Place`]), and no step is a key of the request: a step is a field or a
//! variant the reading type declares, an index, or `*` for an entry under a
//! key that is none of those. Where serde reads an object into a buffer
//! before it reads it (the body of an internally tagged enum), a fault
//! inside is placed at that object.
//!
//! The second read decides nothing: which requests are refused stays
//! `serde_json`'s. It reads a value as `serde_json` reads it, so that it
//! finds the fault `serde_json` found, with two exceptions, for types the
//! decision verbs do not use: a `serde_json::value::RawValue` is not read,
//! and a map whose keys are not strings is refused at its first key
//! (`serde_json` reads the key `"7"` as the number 7; the second read hands
//! every key over as a string). For such a type the second read may name
//! another place than `serde_json`'s fault; it still repeats nothing of the
//! request and decides nothing. A second read that finds no fault, or fails
//! in a way of its own, names none: the message is then the plain `invalid
//! params`.
//!
//! The read touches nothing but the params it is given. It walks the value
//! `serde_json` walked, no deeper, and builds what `serde_json` built up to
//! the fault. The message is the place and one fault in fixed words: its
//! length is bounded by the reading type (its names, its nesting, and the
//! digits of an index or a length), not by the request.

use std::fmt;

use serde::de::value::{BorrowedStrDeserializer, MapAccessDeserializer};
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, Deserializer, Expected, MapAccess, SeqAccess,
    Unexpected, Visitor,
};
use serde::forward_to_deserialize_any;
use serde_json::{Map, Value};

/// The invalid params message for `params`, which did not read as `T`.
pub(super) fn message<T: DeserializeOwned>(params: &Value) -> String {
    let read = T::deserialize(Read {
        value: params,
        place: &Place::Params,
    });
    match read {
        Err(fault) if !fault.of_the_read => {
            format!("invalid params: {}", fault.at(&Place::Params))
        }
        // The second read found nothing to name, or failed by itself: say
        // only what was refused.
        _ => "invalid params".to_owned(),
    }
}

/// Where a value lies in the params: the params themselves, or one step
/// below another place. A named step is a field or a variant the reading
/// type declares, or `*`; it is never a key as the request wrote it.
#[derive(Clone, Copy)]
enum Place<'p> {
    Params,
    Entry(&'p Place<'p>, &'static str),
    Item(&'p Place<'p>, usize),
}

impl fmt::Display for Place<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Place::Params => Ok(()),
            Place::Entry(Place::Params, name) => formatter.write_str(name),
            Place::Entry(parent, name) => write!(formatter, "{parent}.{name}"),
            Place::Item(parent, index) => write!(formatter, "{parent}[{index}]"),
        }
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
    /// The second read failed by itself: nothing of this fault is said.
    of_the_read: bool,
}

impl Fault {
    fn new(what: String) -> Self {
        Self {
            path: None,
            field: None,
            what,
            of_the_read: false,
        }
    }

    fn of_field(field: &'static str, what: &str) -> Self {
        Self {
            field: Some(field),
            ..Self::new(what.to_owned())
        }
    }

    fn of_the_read() -> Self {
        Self {
            of_the_read: true,
            ..Self::new(String::new())
        }
    }

    /// This fault, at `place` unless a deeper level placed it already.
    fn at(mut self, place: &Place<'_>) -> Self {
        self.path.get_or_insert_with(|| place.to_string());
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

/// The names a reading type accepts, in serde's words.
fn one_of(names: &'static [&'static str], none: &str) -> String {
    match names {
        [] => format!("there are no {none}"),
        [only] => format!("expected `{only}`"),
        [first, second] => format!("expected `{first}` or `{second}`"),
        names => format!("expected one of `{}`", names.join("`, `")),
    }
}

/// How the entries of an object are stepped to.
#[derive(Clone, Copy)]
enum Keys {
    /// The fields of a struct or the variants of an enum: an entry is named
    /// when its key is one of them, and is `*` otherwise.
    Declared(&'static [&'static str]),
    /// A map keyed by the request: every entry is `*`.
    Free,
    /// An object read with no type known for it, as serde reads one into a
    /// buffer: a fault in an entry is placed at the object.
    Unread,
}

/// One value of the params, read where it lies, and its place in them.
struct Read<'p, 'de> {
    value: &'de Value,
    place: &'p Place<'p>,
}

impl<'p, 'de> Read<'p, 'de> {
    fn entries(&self, entries: &'de Map<String, Value>, keys: Keys) -> Entries<'p, 'de> {
        Entries {
            place: self.place,
            keys,
            entries: entries.iter(),
            pending: None,
        }
    }

    /// The visitor's reading of an object. One it left entries of is
    /// refused in `serde_json`'s words for it.
    fn object<V: Visitor<'de>>(
        &self,
        entries: &'de Map<String, Value>,
        keys: Keys,
        visitor: V,
    ) -> Result<V::Value, Fault> {
        let mut rest = self.entries(entries, keys);
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
                (None, None, None) => Err(Fault::of_the_read()),
            },
            Value::String(value) => visitor.visit_borrowed_str(value),
            // An array the visitor left elements of is refused in
            // `serde_json`'s words for it.
            Value::Array(items) => {
                let mut rest = Items {
                    place: self.place,
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
            Value::Object(entries) => self.object(entries, Keys::Unread, visitor),
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

    /// A struct is an object of its fields, or an array of them in order.
    /// Anything else is refused without the struct's Rust name.
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Fault> {
        match self.value {
            Value::Object(entries) => self.object(entries, Keys::Declared(fields), visitor),
            Value::Array(_) => self.deserialize_any(visitor),
            _ => Err(Fault::new("expected an object".to_owned())),
        }
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Fault> {
        match self.value {
            Value::Object(entries) => self.object(entries, Keys::Free, visitor),
            _ => self.deserialize_any(visitor),
        }
    }

    /// A variant is its name, or an object of the one entry that names it;
    /// anything else is refused in `serde_json`'s words for it.
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Fault> {
        match self.value {
            Value::String(variant) => visitor.visit_enum(BorrowedStrDeserializer::new(variant)),
            Value::Object(entries) if entries.len() == 1 => visitor.visit_enum(
                MapAccessDeserializer::new(self.entries(entries, Keys::Declared(variants))),
            ),
            Value::Object(_) => Err(de::Error::invalid_value(
                Unexpected::Map,
                &"map with a single key",
            )),
            other => Err(Fault::new(format!(
                "invalid type: {}, expected string or map",
                shape(other)
            ))),
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
        bytes byte_buf unit unit_struct seq tuple tuple_struct ignored_any
    }
}

/// The entries of one object of the params. A fault in a value is placed at
/// the step [`Keys`] gives its entry; a fault in a key is left to the
/// object's own place.
struct Entries<'p, 'de> {
    place: &'p Place<'p>,
    keys: Keys,
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
            return Err(Fault::of_the_read());
        };
        let step = match self.keys {
            Keys::Declared(names) => Some(
                names
                    .iter()
                    .copied()
                    .find(|name| *name == key)
                    .unwrap_or("*"),
            ),
            Keys::Free => Some("*"),
            Keys::Unread => None,
        };
        let place = step.map_or(*self.place, |step| Place::Entry(self.place, step));
        seed.deserialize(Read {
            value,
            place: &place,
        })
        .map_err(|fault| fault.at(&place))
    }
}

/// The elements of one array of the params, each placed at its index.
struct Items<'p, 'de> {
    place: &'p Place<'p>,
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
        let place = Place::Item(self.place, index);
        seed.deserialize(Read {
            value,
            place: &place,
        })
        .map(Some)
        .map_err(|fault| fault.at(&place))
    }
}
