use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Map as JsonMap;
use serde_json::Value as JsonValue;

use crate::script::FunctionBody;
use crate::script::json_value_from_number;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RuntimeValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<RuntimeValue>),
    Object(HashMap<String, RuntimeValue>),
    Function(Arc<FunctionValue>),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FunctionValue {
    pub(crate) params: Vec<String>,
    pub(crate) body: FunctionBody,
    pub(crate) captured_scopes: Vec<HashMap<String, RuntimeValue>>,
}

impl RuntimeValue {
    pub(crate) fn from_json(value: JsonValue) -> Self {
        match value {
            JsonValue::Null => Self::Null,
            JsonValue::Bool(value) => Self::Bool(value),
            JsonValue::Number(value) => Self::Number(value.as_f64().unwrap_or_default()),
            JsonValue::String(value) => Self::String(value),
            JsonValue::Array(values) => {
                Self::Array(values.into_iter().map(Self::from_json).collect())
            }
            JsonValue::Object(values) => Self::Object(
                values
                    .into_iter()
                    .map(|(key, value)| (key, Self::from_json(value)))
                    .collect(),
            ),
        }
    }

    pub(crate) fn to_json(&self) -> Result<JsonValue, String> {
        match self {
            Self::Null => Ok(JsonValue::Null),
            Self::Bool(value) => Ok(JsonValue::Bool(*value)),
            Self::Number(value) => Ok(json_value_from_number(*value)),
            Self::String(value) => Ok(JsonValue::String(value.clone())),
            Self::Array(values) => values
                .iter()
                .map(Self::to_json)
                .collect::<Result<Vec<_>, _>>()
                .map(JsonValue::Array),
            Self::Object(values) => {
                let mut object = JsonMap::new();
                for (key, value) in values {
                    object.insert(key.clone(), value.to_json()?);
                }
                Ok(JsonValue::Object(object))
            }
            Self::Function(_) => Err(
                "workflow result must be structured-cloneable; did you forget to await agent(), parallel(), or pipeline()? function value cannot be returned"
                    .to_string(),
            ),
        }
    }

    pub(crate) fn as_string(&self) -> Option<String> {
        match self {
            Self::String(value) => Some(value.clone()),
            _ => None,
        }
    }

    pub(crate) fn as_number(&self) -> f64 {
        match self {
            Self::Number(value) => *value,
            Self::Bool(true) => 1.0,
            Self::Bool(false) | Self::Null => 0.0,
            Self::String(value) => value.parse::<f64>().unwrap_or_default(),
            Self::Array(_) | Self::Object(_) | Self::Function(_) => 0.0,
        }
    }

    pub(crate) fn is_truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(value) => *value,
            Self::Number(value) => *value != 0.0 && !value.is_nan(),
            Self::String(value) => !value.is_empty(),
            Self::Array(_) | Self::Object(_) | Self::Function(_) => true,
        }
    }

    pub(crate) fn is_string_like(&self) -> bool {
        matches!(self, Self::String(_))
    }

    pub(crate) fn to_display_string(&self) -> String {
        match self {
            Self::Null => "null".to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Number(value) => json_value_from_number(*value).to_string(),
            Self::String(value) => value.clone(),
            Self::Array(_) | Self::Object(_) => self
                .to_json()
                .and_then(|value| serde_json::to_string(&value).map_err(|err| err.to_string()))
                .unwrap_or_else(|_| "[object Object]".to_string()),
            Self::Function(_) => "function".to_string(),
        }
    }

    pub(crate) fn member(&self, property: &str) -> Option<Self> {
        match self {
            Self::Object(values) => values.get(property).cloned(),
            Self::Array(values) if property == "length" => Some(Self::Number(values.len() as f64)),
            _ => None,
        }
    }

    pub(crate) fn index(&self, index: &Self) -> Option<Self> {
        match (self, index) {
            (Self::Array(values), Self::Number(index)) => values.get(*index as usize).cloned(),
            (Self::Object(values), Self::String(key)) => values.get(key).cloned(),
            (Self::String(value), Self::Number(index)) => value
                .chars()
                .nth(*index as usize)
                .map(|ch| Self::String(ch.to_string())),
            _ => None,
        }
    }
}
