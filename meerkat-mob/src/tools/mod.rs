pub mod mob_tools;
pub mod task_tools;

pub(crate) fn empty_object_schema() -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "type".to_string(),
        serde_json::Value::String("object".to_string()),
    );
    obj.insert(
        "properties".to_string(),
        serde_json::Value::Object(serde_json::Map::new()),
    );
    obj.insert(
        "required".to_string(),
        serde_json::Value::Array(Vec::new()),
    );
    serde_json::Value::Object(obj)
}

pub use mob_tools::MobToolDispatcher;
pub use task_tools::MobTaskToolDispatcher;
