use agent_client_protocol::schema::v2::Meta;
use serde::{Serialize, de::DeserializeOwned};

/// Serialize metadata, optionally under an extension namespace. Empty objects are omitted.
pub fn to_meta<T: Serialize>(value: &T, namespace: Option<&str>) -> Option<Meta> {
    let serde_json::Value::Object(map) = serde_json::to_value(value).expect("metadata should serialize") else {
        panic!("metadata must serialize as an object");
    };
    if map.is_empty() {
        return None;
    }
    Some(match namespace {
        Some(namespace) => Meta::from_iter([(namespace.to_owned(), serde_json::Value::Object(map))]),
        None => map,
    })
}

/// Decode metadata, treating absent or invalid values as the type's default.
pub fn from_meta<T: DeserializeOwned + Default>(meta: Option<&Meta>, namespace: Option<&str>) -> T {
    meta.and_then(|meta| match namespace {
        Some(namespace) => meta.get(namespace).cloned(),
        None => Some(serde_json::Value::Object(meta.clone())),
    })
    .and_then(|value| serde_json::from_value(value).ok())
    .unwrap_or_default()
}
