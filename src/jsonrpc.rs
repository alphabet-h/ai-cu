//! JSON-RPC 2.0 の 1 行のメッセージの解釈と、応答の組み立て (MCP の stdio 用。D-055)。
//! MCP の制約: リクエストの id は文字列か整数で、null は不可。通知には id が無い。
//! 出力は `serde_json::to_string` で 1 行にする (文字列の中の改行はエスケープされるので、メッセージに生の改行は入らない)。

use serde_json::{Value, json};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
/// MCP 2026-07-28: 対応していない版 (`data` に `supported` と `requested`)
pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;

#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// 1 行の中身。配列は旧版 (2025-03-26 以前) のバッチ。
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    One(Value),
    Batch(Vec<Value>),
}

/// 1 行を JSON として読む。読めなければ -32700、空の配列は -32600。
pub fn parse_line(line: &str) -> Result<Incoming, RpcError> {
    let v: Value = serde_json::from_str(line)
        .map_err(|e| RpcError::new(PARSE_ERROR, format!("JSON として読めない: {e}")))?;
    match v {
        Value::Array(items) if items.is_empty() => {
            Err(RpcError::new(INVALID_REQUEST, "空のバッチ"))
        }
        Value::Array(items) => Ok(Incoming::Batch(items)),
        other => Ok(Incoming::One(other)),
    }
}

/// id として有効か (文字列か整数。MCP では null は不可)。
fn valid_id(v: &Value) -> bool {
    match v {
        Value::String(_) => true,
        Value::Number(n) => n.is_i64() || n.is_u64(),
        _ => false,
    }
}

/// 1 つのメッセージ (オブジェクト) をリクエストか通知として解釈する。
pub fn parse_message(v: &Value) -> Result<Message, RpcError> {
    let obj = v
        .as_object()
        .ok_or_else(|| RpcError::new(INVALID_REQUEST, "メッセージがオブジェクトでない"))?;
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(RpcError::new(INVALID_REQUEST, "jsonrpc が \"2.0\" でない"));
    }
    let method = obj
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(INVALID_REQUEST, "method が文字列でない"))?
        .to_string();
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    match obj.get("id") {
        None => Ok(Message::Notification { method, params }),
        Some(id) if valid_id(id) => Ok(Message::Request {
            id: id.clone(),
            method,
            params,
        }),
        Some(_) => Err(RpcError::new(
            INVALID_REQUEST,
            "id は文字列か整数 (null は不可)",
        )),
    }
}

/// 応答に使う id。リクエストの id が読めれば (文字列か整数) それ、読めなければ None (null で返す)。
pub fn id_of(v: &Value) -> Option<Value> {
    v.get("id").filter(|id| valid_id(id)).cloned()
}

pub fn response(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

pub fn error_response(id: Option<&Value>, e: &RpcError) -> Value {
    let mut err = json!({ "code": e.code, "message": e.message });
    if let Some(d) = &e.data {
        err["data"] = d.clone();
    }
    json!({ "jsonrpc": "2.0", "id": id.cloned().unwrap_or(Value::Null), "error": err })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonrpc_parses_requests_with_string_or_integer_ids_and_notifications() {
        let r = parse_message(&json!({"jsonrpc":"2.0","id":1,"method":"ping"})).unwrap();
        assert_eq!(
            r,
            Message::Request {
                id: json!(1),
                method: "ping".into(),
                params: Value::Null
            }
        );
        let r = parse_message(
            &json!({"jsonrpc":"2.0","id":"abc","method":"tools/call","params":{"name":"x"}}),
        )
        .unwrap();
        assert!(matches!(r, Message::Request { id, .. } if id == json!("abc")));
        let n =
            parse_message(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).unwrap();
        assert_eq!(
            n,
            Message::Notification {
                method: "notifications/initialized".into(),
                params: Value::Null
            }
        );
    }

    #[test]
    fn jsonrpc_rejects_null_id_missing_version_and_non_objects() {
        for bad in [
            json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
            json!({"jsonrpc":"2.0","id":1.5,"method":"ping"}),
            json!({"jsonrpc":"2.0","id":true,"method":"ping"}),
            json!({"id":1,"method":"ping"}),
            json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
            json!({"jsonrpc":"2.0","id":1}),
            json!({"jsonrpc":"2.0","id":1,"method":7}),
            json!(1),
            json!("x"),
        ] {
            let e = parse_message(&bad).unwrap_err();
            assert_eq!(e.code, INVALID_REQUEST, "{bad}");
        }
    }

    #[test]
    fn jsonrpc_parse_line_distinguishes_batch_and_reports_parse_errors() {
        assert_eq!(parse_line("{").unwrap_err().code, PARSE_ERROR);
        assert_eq!(parse_line("").unwrap_err().code, PARSE_ERROR);
        assert_eq!(parse_line("[]").unwrap_err().code, INVALID_REQUEST);
        assert_eq!(
            parse_line(r#"[{"a":1},{"b":2}]"#).unwrap(),
            Incoming::Batch(vec![json!({"a":1}), json!({"b":2})])
        );
        assert_eq!(
            parse_line(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).unwrap(),
            Incoming::One(json!({"jsonrpc":"2.0","id":1,"method":"ping"}))
        );
    }

    #[test]
    fn jsonrpc_responses_echo_id_and_errors_carry_code_message_data() {
        let r = response(&json!("q-1"), json!({"ok": true}));
        assert_eq!(r, json!({"jsonrpc":"2.0","id":"q-1","result":{"ok":true}}));
        let e = error_response(
            Some(&json!(3)),
            &RpcError::new(METHOD_NOT_FOUND, "unknown").with_data(json!({"x": 1})),
        );
        assert_eq!(
            e,
            json!({"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"unknown","data":{"x":1}}})
        );
        let e = error_response(None, &RpcError::new(PARSE_ERROR, "bad"));
        assert_eq!(e["id"], Value::Null);
        assert!(e["error"].get("data").is_none());
        // id が読めないリクエスト (null) の応答は null、読めるものはそのまま
        assert_eq!(id_of(&json!({"id": null})), None);
        assert_eq!(id_of(&json!({"id": "s"})), Some(json!("s")));
        assert_eq!(id_of(&json!({"id": 2})), Some(json!(2)));
        assert_eq!(id_of(&json!({})), None);
    }

    #[test]
    fn jsonrpc_serialized_message_has_no_raw_newline() {
        let r = response(&json!(1), json!({"text": "a\nb\r\nc"}));
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains('\n') && !s.contains('\r'), "{s}");
        assert!(s.contains("\\n"));
    }
}
