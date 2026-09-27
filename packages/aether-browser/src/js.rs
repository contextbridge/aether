use crate::ClientError;
use serde::Serialize;
use serde::de::DeserializeOwned;
use wasm_bindgen::JsValue;

pub(crate) fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, ClientError> {
    serde_wasm_bindgen::from_value(value).map_err(ClientError::InvalidArgument)
}

pub(crate) fn to_js<T: Serialize>(value: &T) -> Result<JsValue, ClientError> {
    value.serialize(&serde_wasm_bindgen::Serializer::json_compatible()).map_err(ClientError::Conversion)
}
