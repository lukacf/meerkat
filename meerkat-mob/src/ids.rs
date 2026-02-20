//! Newtype identifiers for mob entities.
//!
//! These types wrap concrete primitives for compile-time safety.

use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

/// Unique identifier for a flow run.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(Uuid);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for RunId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

macro_rules! string_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

string_newtype!(
    /// Unique identifier for a mob instance.
    MobId
);

string_newtype!(
    /// Unique identifier for a flow definition.
    FlowId
);

string_newtype!(
    /// Unique identifier for a step in a flow definition.
    StepId
);

string_newtype!(
    /// Unique identifier for a meerkat (agent instance) within a mob.
    MeerkatId
);

string_newtype!(
    /// Profile name within a mob definition.
    ProfileName
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_run_id_roundtrip_json() {
        let run_id = RunId::new();
        let encoded = serde_json::to_string(&run_id).unwrap();
        let decoded: RunId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, run_id);
    }

    #[test]
    fn test_run_id_roundtrip_parse_display() {
        let run_id = RunId::new();
        let rendered = run_id.to_string();
        let reparsed = RunId::from_str(&rendered).unwrap();
        assert_eq!(reparsed, run_id);
    }

    #[test]
    fn test_flow_id_roundtrip_json() {
        let id = FlowId::from("flow-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: FlowId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_step_id_roundtrip_json() {
        let id = StepId::from("step-a");
        let encoded = serde_json::to_string(&id).unwrap();
        let decoded: StepId = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, id);
    }

    #[test]
    fn test_existing_ids_roundtrip() {
        let mob = MobId::from("mob-a");
        let meerkat = MeerkatId::from("meerkat-a");
        let profile = ProfileName::from("lead");
        assert_eq!(
            serde_json::from_str::<MobId>(&serde_json::to_string(&mob).unwrap()).unwrap(),
            mob
        );
        assert_eq!(
            serde_json::from_str::<MeerkatId>(&serde_json::to_string(&meerkat).unwrap()).unwrap(),
            meerkat
        );
        assert_eq!(
            serde_json::from_str::<ProfileName>(&serde_json::to_string(&profile).unwrap()).unwrap(),
            profile
        );
    }
}
