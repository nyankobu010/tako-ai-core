//! Python ↔ Rust conversion helpers.
//!
//! Several helpers (`json_to_py`, `py_to_json`, `messages_from`) are
//! reserved for the Phase 1.5 follow-up (custom Python providers, MCP
//! bindings) and are currently dead code; the lint is silenced module-wide.
#![allow(dead_code)]

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use serde_json::Value;
use tako_core::{Message, Principal, Role};

/// Convert a `serde_json::Value` to a Python object (None / bool / int /
/// float / str / list / dict).
pub fn json_to_py<'py>(py: Python<'py>, v: &Value) -> PyResult<Bound<'py, PyAny>> {
    match v {
        Value::Null => Ok(py.None().into_bound(py)),
        Value::Bool(b) => Ok(b.into_pyobject(py)?.to_owned().into_any()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i.into_pyobject(py)?.to_owned().into_any())
            } else if let Some(f) = n.as_f64() {
                Ok(f.into_pyobject(py)?.to_owned().into_any())
            } else {
                Ok(py.None().into_bound(py))
            }
        }
        Value::String(s) => Ok(s.into_pyobject(py)?.to_owned().into_any()),
        Value::Array(arr) => {
            let list = PyList::empty(py);
            for item in arr {
                let py_item = json_to_py(py, item)?;
                list.append(py_item)?;
            }
            Ok(list.into_any())
        }
        Value::Object(map) => {
            let dict = PyDict::new(py);
            for (k, val) in map {
                dict.set_item(k, json_to_py(py, val)?)?;
            }
            Ok(dict.into_any())
        }
    }
}

/// Convert a Python object to `serde_json::Value`. Accepts dicts, lists,
/// strings, ints, floats, bools, None.
pub fn py_to_json(obj: &Bound<'_, PyAny>) -> PyResult<Value> {
    if obj.is_none() {
        return Ok(Value::Null);
    }
    if let Ok(b) = obj.extract::<bool>() {
        return Ok(Value::Bool(b));
    }
    if let Ok(i) = obj.extract::<i64>() {
        return Ok(Value::Number(i.into()));
    }
    if let Ok(f) = obj.extract::<f64>() {
        return Ok(serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null));
    }
    if let Ok(s) = obj.extract::<String>() {
        return Ok(Value::String(s));
    }
    if let Ok(list) = obj.cast::<PyList>() {
        let mut out = Vec::with_capacity(list.len());
        for item in list.iter() {
            out.push(py_to_json(&item)?);
        }
        return Ok(Value::Array(out));
    }
    if let Ok(dict) = obj.cast::<PyDict>() {
        let mut out = serde_json::Map::with_capacity(dict.len());
        for (k, v) in dict.iter() {
            let key: String = k.extract()?;
            out.insert(key, py_to_json(&v)?);
        }
        return Ok(Value::Object(out));
    }
    Err(pyo3::exceptions::PyTypeError::new_err(
        "unsupported type for JSON conversion (expected None / bool / int / float / str / list / dict)",
    ))
}

/// Build a [`Principal`] from kwargs-style optional strings.
pub fn principal_from(tenant: Option<&str>, user: Option<&str>) -> Principal {
    let mut p = Principal::anonymous();
    if let Some(t) = tenant {
        p.tenant_id = t.to_string();
    }
    if let Some(u) = user {
        p.user_id = u.to_string();
    }
    p
}

/// Convert a list of (role, text) tuples into Vec<Message>.
pub fn messages_from(pairs: Vec<(String, String)>) -> Vec<Message> {
    pairs
        .into_iter()
        .map(|(role, text)| {
            let role = match role.as_str() {
                "system" => Role::System,
                "assistant" => Role::Assistant,
                "tool" => Role::Tool,
                _ => Role::User,
            };
            Message {
                role,
                content: vec![tako_core::ContentPart::Text { text }],
            }
        })
        .collect()
}
