//! Minimal JSON-RPC 2.0 wire types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Debug)]
pub struct Request<'a> {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: &'a str,
    pub params: Value,
}

#[derive(Serialize, Debug)]
pub struct Notification<'a> {
    pub jsonrpc: &'static str,
    pub method: &'a str,
    pub params: Value,
}

#[derive(Deserialize, Debug)]
pub struct Response {
    #[allow(dead_code)]
    pub jsonrpc: Option<String>,
    pub id: Option<u64>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<RpcError>,
    /// Server → client notifications carry `method` and no `id`.
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Deserialize, Debug)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub data: Option<Value>,
}

pub fn request(id: u64, method: &str, params: Value) -> String {
    let req = Request {
        jsonrpc: "2.0",
        id,
        method,
        params,
    };
    serde_json::to_string(&req).unwrap_or_default()
}

pub fn notification(method: &str, params: Value) -> String {
    let n = Notification {
        jsonrpc: "2.0",
        method,
        params,
    };
    serde_json::to_string(&n).unwrap_or_default()
}
