//! Shared wire constraints used by generated code.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use serde_json::Value;
use std::{collections::BTreeMap, fmt};

/// A credential value. Debug is redacted; serialization requires deliberate use.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl From<String> for SecretString {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for SecretString {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString([REDACTED])")
    }
}

/// Telegram's literal `True`, rather than an arbitrary boolean.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct True;

impl Serialize for True {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for True {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(D::Error::custom("expected literal true"))
        }
    }
}

/// Telegram's literal integer zero for an inaccessible message's date.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Zero;

impl Serialize for Zero {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_i64(0)
    }
}

impl<'de> Deserialize<'de> for Zero {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if i64::deserialize(deserializer)? == 0 {
            Ok(Self)
        } else {
            Err(D::Error::custom("expected literal zero"))
        }
    }
}

/// A numeric chat ID or Telegram username. UserId does not convert to ChatTarget.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ChatTarget {
    Id(crate::ChatId),
    Username(String),
}

impl From<crate::ChatId> for ChatTarget {
    fn from(value: crate::ChatId) -> Self {
        Self::Id(value)
    }
}

impl From<String> for ChatTarget {
    fn from(value: String) -> Self {
        Self::Username(value)
    }
}

impl From<&str> for ChatTarget {
    fn from(value: &str) -> Self {
        Self::Username(value.into())
    }
}

/// A future tagged variant, retained for inspection and lossless forwarding.
/// Its contents are data, not an instruction or an authorization grant.
#[derive(Clone, Serialize)]
#[serde(transparent)]
pub struct UnknownVariant(Value);

impl UnknownVariant {
    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub(crate) fn new(value: Value) -> Self {
        Self(value)
    }
}

impl fmt::Debug for UnknownVariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnknownVariant").finish_non_exhaustive()
    }
}

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct UnknownFields(BTreeMap<String, Value>);

impl UnknownFields {
    pub(crate) fn as_map(&self) -> &BTreeMap<String, Value> {
        &self.0
    }
}

impl fmt::Debug for UnknownFields {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnknownFields")
            .field("count", &self.0.len())
            .finish_non_exhaustive()
    }
}

pub(crate) struct ObjectShape {
    pub required: &'static [&'static str],
    pub allowed: &'static [&'static str],
}

/// Select one complete shape without swallowing another candidate's known fields.
pub(crate) fn select_shape(value: &Value, shapes: &[ObjectShape]) -> Result<usize, &'static str> {
    let object = value.as_object().ok_or("expected an object union")?;
    let mut selected = None;
    for (index, shape) in shapes.iter().enumerate() {
        if !shape
            .required
            .iter()
            .all(|field| object.contains_key(*field))
        {
            continue;
        }
        if object.keys().any(|key| {
            !shape.allowed.contains(&key.as_str())
                && shapes
                    .iter()
                    .any(|other| other.allowed.contains(&key.as_str()))
        }) {
            continue;
        }
        if selected.replace(index).is_some() {
            return Err("ambiguous object union");
        }
    }
    selected.ok_or("incomplete or inconsistent object union")
}
