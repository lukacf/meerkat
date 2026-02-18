use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct MobId(String);

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct MeerkatId(String);

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct ProfileName(String);

impl MobId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl MeerkatId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl ProfileName {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for MobId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for MeerkatId {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl Borrow<str> for ProfileName {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for MobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl fmt::Display for MeerkatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

impl From<&str> for MobId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for MobId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for MeerkatId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for MeerkatId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for ProfileName {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ProfileName {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl FromStr for MobId {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(value))
    }
}

impl FromStr for MeerkatId {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(value))
    }
}

impl FromStr for ProfileName {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(value))
    }
}
