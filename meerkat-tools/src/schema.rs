//! Shared JSON schema helpers for tool definitions.

use serde_json::{Map, Value, json};

#[derive(Debug, Default)]
pub struct SchemaBuilder {
    properties: Map<String, Value>,
    required: Vec<String>,
}

impl SchemaBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn property(mut self, name: impl Into<String>, schema: Value) -> Self {
        self.properties.insert(name.into(), schema);
        self
    }

    pub fn required(mut self, name: impl Into<String>) -> Self {
        self.required.push(name.into());
        self
    }

    pub fn build(self) -> Value {
        json!({
            "type": "object",
            "properties": self.properties,
            "required": self.required,
        })
    }
}

pub fn empty_object_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "required": [],
    })
}
