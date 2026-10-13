use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

pub fn to_meta<T: Serialize>(value: &T, namespace: Option<&str>) -> Option<Map<String, Value>> {
    let Value::Object(map) = serde_json::to_value(value).expect("metadata should serialize") else {
        panic!("metadata must serialize as an object");
    };
    if map.is_empty() {
        return None;
    }
    Some(match namespace {
        Some(namespace) => Map::from_iter([(namespace.to_owned(), Value::Object(map))]),
        None => map,
    })
}

pub fn from_meta<T: DeserializeOwned + Default>(meta: Option<&Map<String, Value>>, namespace: Option<&str>) -> T {
    meta.and_then(|meta| match namespace {
        Some(namespace) => meta.get(namespace).cloned(),
        None => Some(Value::Object(meta.clone())),
    })
    .and_then(|value| serde_json::from_value(value).ok())
    .unwrap_or_default()
}
