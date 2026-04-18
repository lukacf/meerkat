use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::HashMap;

pub const DEFAULT_MODEL: &str = "gpt-realtime";

/// Arbitrary JSON payloads allowed by the API (e.g. metadata values).
pub type Metadata = HashMap<String, Value>;

/// JSON Schema / tool parameter definitions are intentionally untyped.
pub type JsonSchema = Value;

/// Free-form JSON payloads where the spec is open-ended.
pub type ArbitraryJson = Value;

/// Tri-state helper for fields that can be omitted, set to null, or set to a value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Nullable<T> {
    Value(T),
    Null,
}

impl<T> Nullable<T> {
    #[must_use]
    pub const fn as_ref(&self) -> Option<&T> {
        match self {
            Self::Value(value) => Some(value),
            Self::Null => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[default]
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ItemStatus {
    #[default]
    InProgress,
    Completed,
    Incomplete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    #[default]
    Audio,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputModalities {
    Audio,
    Text,
}

impl Serialize for OutputModalities {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let values = match self {
            Self::Audio => vec![Modality::Audio],
            Self::Text => vec![Modality::Text],
        };
        values.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for OutputModalities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Single(Modality),
            Many(Vec<Modality>),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Single(Modality::Audio) => Ok(Self::Audio),
            Repr::Single(Modality::Text) => Ok(Self::Text),
            Repr::Many(values) => match values.as_slice() {
                [Modality::Audio] => Ok(Self::Audio),
                [Modality::Text] => Ok(Self::Text),
                _ => Err(serde::de::Error::custom(
                    "output_modalities must contain exactly one of: audio or text",
                )),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Eagerness {
    Auto,
    Low,
    #[default]
    Medium,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Voice {
    Id(String),
    Object { id: String },
}

impl<S: Into<String>> From<S> for Voice {
    fn from(s: S) -> Self {
        // Own the string to avoid lifetime plumbing in public APIs.
        Self::Id(s.into())
    }
}

impl std::fmt::Display for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Id(id) | Self::Object { id } => write!(f, "{id}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum MaxTokens {
    Count(u32),
    Infinite(Infinite),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Infinite {
    #[serde(rename = "inf")]
    Inf,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
#[serde(transparent)]
pub struct Temperature(f32);

impl Temperature {
    /// # Errors
    /// Returns an error if `val` is outside the inclusive range [0.0, 2.0].
    pub fn new(val: f32) -> Result<Self, TemperatureError> {
        if (0.0..=2.0).contains(&val) {
            Ok(Self(val))
        } else {
            Err(TemperatureError { value: val })
        }
    }
}

impl Default for Temperature {
    fn default() -> Self {
        Self(0.8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TemperatureError {
    pub value: f32,
}

impl std::fmt::Display for TemperatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "temperature must be between 0.0 and 2.0, got {}",
            self.value
        )
    }
}

impl std::error::Error for TemperatureError {}

impl TryFrom<f32> for Temperature {
    type Error = TemperatureError;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for Temperature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = f32::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum PromptRef {
    Id(String),
    Object { id: String },
}
