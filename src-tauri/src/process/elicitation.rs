//! MCP elicitation is not a permission approval. Never synthesize proof or
//! coerce user input, and fail closed for schema features we cannot validate.
use crate::model::Interaction;
use serde_json::{json, Value};

#[derive(Debug, PartialEq)]
pub enum Inbound {
    Notification,
    Request,
    Response,
    Malformed,
}
pub fn classify(v: &Value) -> Inbound {
    let id = v.get("id");
    let valid_id = id.is_some_and(|v| v.is_string() || v.is_i64() || v.is_u64());
    if !v.is_object() {
        return Inbound::Malformed;
    }
    if let Some(method) = v.get("method") {
        if !method.is_string() || v.get("result").is_some() || v.get("error").is_some() {
            return Inbound::Malformed;
        }
        return match id {
            None => Inbound::Notification,
            Some(_) if valid_id => Inbound::Request,
            _ => Inbound::Malformed,
        };
    }
    if valid_id && (v.get("result").is_some() ^ v.get("error").is_some()) {
        Inbound::Response
    } else {
        Inbound::Malformed
    }
}

pub fn request(v: &Value) -> Result<Interaction, String> {
    let p = &v["params"];
    if !p.is_object() {
        return Err("MCP 请求缺少 params 对象".into());
    }
    let mode = p["mode"].as_str().unwrap_or("form");
    let schema = &p["requestedSchema"];
    let (kind, issue) = match mode {
        "form" | "openai/form" | "openaiForm" => match supported(schema) {
            Ok(()) => (
                if schema["properties"]
                    .as_object()
                    .is_some_and(|p| p.is_empty())
                {
                    "mcp_confirmation"
                } else {
                    "mcp_form"
                },
                None,
            ),
            Err(e) => ("mcp_unsupported", Some(e)),
        },
        "url" if p["url"].as_str().is_some_and(safe_url) => ("mcp_url", None),
        "url" => (
            "mcp_unsupported",
            Some("链接必须是有效的 http/https URL；请拒绝或取消后检查服务端配置。".into()),
        ),
        _ => (
            "mcp_unsupported",
            Some(format!(
                "尚不支持 MCP 模式 {mode}，不能生成身份验证证明；请拒绝或取消并人工处理。"
            )),
        ),
    };
    Ok(Interaction {
        id: uuid::Uuid::new_v4().to_string(),
        protocol_id: Some(v["id"].to_string()),
        title: format!(
            "MCP 用户交互 · {}",
            p["serverName"].as_str().unwrap_or("未知服务")
        ),
        detail: format!(
            "{}{}",
            p["message"]
                .as_str()
                .or(p["description"].as_str())
                .unwrap_or("请确认此请求"),
            issue.map(|e| format!("\n{e}")).unwrap_or_default()
        ),
        questions: vec![],
        approval: false,
        can_remember: false,
        kind: kind.into(),
        mcp_mode: Some(mode.into()),
        mcp_schema: if kind == "mcp_form" || kind == "mcp_confirmation" {
            Some(schema.clone())
        } else {
            None
        },
        mcp_url: p["url"].as_str().map(str::to_owned),
        status: "pending".into(),
        mcp_context: Some(
            json!({"threadId":p["threadId"],"turnId":p["turnId"],"serverName":p["serverName"],"elicitationId":p["elicitationId"]}),
        ),
        policy: Default::default(),
        decision: None,
        delivery: Some("pending".into()),
    })
}

fn safe_url(s: &str) -> bool {
    s.strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .is_some_and(|rest| {
            !rest.is_empty() && !rest.starts_with('/') && !rest.chars().any(char::is_whitespace)
        })
}

fn keys(v: &Value, allowed: &[&str]) -> Result<(), String> {
    let object = v.as_object().ok_or("MCP schema 必须是对象")?;
    for (key, value) in object {
        if !allowed.contains(&key.as_str()) && !value.is_null() {
            return Err(format!(
                "尚不支持 schema 约束 {key}，请拒绝或取消后人工处理。"
            ));
        }
    }
    Ok(())
}

pub fn supported(schema: &Value) -> Result<(), String> {
    keys(
        schema,
        &[
            "type",
            "properties",
            "required",
            "title",
            "description",
            "$schema",
            "additionalProperties",
        ],
    )?;
    if schema["type"] != "object" {
        return Err("MCP 表单根类型必须是 object".into());
    }
    if !schema["additionalProperties"].is_null() && !schema["additionalProperties"].is_boolean() {
        return Err("尚不支持 additionalProperties schema".into());
    }
    let props = schema["properties"]
        .as_object()
        .ok_or("MCP 表单缺少 properties")?;
    if props.len() > 64 {
        return Err("MCP 表单字段超过 64 个".into());
    }
    if let Some(required) = schema.get("required").filter(|v| !v.is_null()) {
        if required.as_array().is_none_or(|a| {
            a.iter()
                .any(|v| v.as_str().is_none_or(|s| !props.contains_key(s)))
        }) {
            return Err("MCP required 字段无效".into());
        }
    }
    for field in props.values() {
        for key in ["title", "description"] {
            if !field[key].is_null() && !field[key].is_string() {
                return Err(format!("{key} 必须为文本"));
            }
        }
        keys(
            field,
            &[
                "type",
                "title",
                "description",
                "default",
                "minimum",
                "maximum",
                "minLength",
                "maxLength",
                "enum",
                "enumNames",
                "oneOf",
                "items",
                "minItems",
                "maxItems",
            ],
        )?;
        match field["type"].as_str() {
            Some("string" | "number" | "integer" | "boolean") => {}
            Some("array") => {
                keys(&field["items"], &["type", "enum", "anyOf"])?;
                if !field["items"]["type"].is_null() && field["items"]["type"] != "string" {
                    return Err("枚举数组元素必须为字符串".into());
                }
                if field["items"]["enum"].is_null() && field["items"]["anyOf"].is_null() {
                    return Err("仅支持字符串枚举数组".into());
                }
            }
            _ => return Err("尚不支持嵌套或未知 MCP 字段类型".into()),
        }
        for (owner, name) in [(field, "enum"), (&field["items"], "enum")] {
            if let Some(options) = owner.get(name).filter(|v| !v.is_null()) {
                if options
                    .as_array()
                    .is_none_or(|a| a.is_empty() || a.iter().any(|v| !v.is_string()))
                {
                    return Err("枚举选项必须为字符串".into());
                }
            }
        }
        for options in [&field["oneOf"], &field["items"]["anyOf"]] {
            if !options.is_null() {
                let options = options.as_array().ok_or("枚举选项无效")?;
                if options.is_empty() {
                    return Err("枚举选项为空".into());
                }
                for option in options {
                    keys(option, &["const", "title"])?;
                    if !option["title"].is_null() && !option["title"].is_string() {
                        return Err("枚举标题必须为文本".into());
                    }
                    if !option["const"].is_string() {
                        return Err("枚举值必须是字符串".into());
                    }
                }
            }
        }
        for key in [
            "minimum",
            "maximum",
            "minLength",
            "maxLength",
            "minItems",
            "maxItems",
        ] {
            if !field[key].is_null() && !field[key].is_number() {
                return Err(format!("{key} 必须为数字"));
            }
        }
        for key in ["minLength", "maxLength", "minItems", "maxItems"] {
            if !field[key].is_null() && field[key].as_u64().is_none() {
                return Err(format!("{key} 必须为非负整数"));
            }
        }
    }
    Ok(())
}

pub fn validate(schema: &Value, content: &Value) -> Result<(), String> {
    supported(schema)?;
    let values = content.as_object().ok_or("MCP 表单回答必须是对象")?;
    let props = schema["properties"].as_object().unwrap();
    if values.keys().any(|k| !props.contains_key(k)) {
        return Err("回答包含未请求的字段".into());
    }
    for required in schema["required"].as_array().into_iter().flatten() {
        if !values.contains_key(required.as_str().unwrap()) {
            return Err(format!("缺少必填字段：{}", required.as_str().unwrap()));
        }
    }
    for (key, value) in values {
        let f = &props[key];
        let valid = match f["type"].as_str() {
            Some("string") => value.is_string(),
            Some("boolean") => value.is_boolean(),
            Some("number") => value.is_number(),
            Some("integer") => value.as_f64().is_some_and(|n| n.fract() == 0.0),
            Some("array") => value
                .as_array()
                .is_some_and(|a| a.iter().all(Value::is_string)),
            _ => false,
        };
        if !valid {
            return Err(format!("字段 {key} 类型不正确"));
        }
        for (min, max, n) in [
            ("minimum", "maximum", value.as_f64()),
            (
                "minLength",
                "maxLength",
                value.as_str().map(|s| s.chars().count() as f64),
            ),
            (
                "minItems",
                "maxItems",
                value.as_array().map(|a| a.len() as f64),
            ),
        ] {
            if n.is_some_and(|n| {
                f[min].as_f64().is_some_and(|m| n < m) || f[max].as_f64().is_some_and(|m| n > m)
            }) {
                return Err(format!("字段 {key} 超出允许范围"));
            }
        }
        let check_enum = |field: &Value, v: &Value| -> bool {
            field["enum"].as_array().is_none_or(|a| a.contains(v))
                && field
                    .get("oneOf")
                    .or(field.get("anyOf"))
                    .filter(|v| !v.is_null())
                    .and_then(Value::as_array)
                    .is_none_or(|a| a.iter().any(|o| &o["const"] == v))
        };
        if !check_enum(f, value) {
            return Err(format!("字段 {key} 不在枚举范围内"));
        }
        if let Some(array) = value.as_array() {
            if array
                .iter()
                .enumerate()
                .any(|(i, v)| !check_enum(&f["items"], v) || array[..i].contains(v))
            {
                return Err(format!("字段 {key} 包含无效或重复选项"));
            }
        }
    }
    Ok(())
}

pub fn response(raw: &Value, request: &Interaction, answer: &Value) -> Result<Value, String> {
    let action = answer["action"].as_str().ok_or("请选择接受、拒绝或取消")?;
    if !matches!(action, "accept" | "decline" | "cancel") {
        return Err("MCP action 无效".into());
    }
    let mut result = json!({"action":action});
    if action == "accept" {
        match request.kind.as_str() {
            "mcp_form" | "mcp_confirmation" => {
                validate(&raw["params"]["requestedSchema"], &answer["content"])?;
                result["content"] = answer["content"].clone();
            }
            "mcp_url" => {}
            _ => return Err("不支持此 MCP 模式，不能接受；请拒绝或取消".into()),
        }
    }
    Ok(json!({"id":raw["id"],"result":result}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_classification_and_special_ids() {
        assert_eq!(
            classify(&json!({"method":"mcpServer/elicitation/request"})),
            Inbound::Notification
        );
        for id in [
            json!(0),
            json!(-1),
            json!("init"),
            json!("thread"),
            json!("引号\"\n"),
            json!(u64::MAX),
        ] {
            let raw = json!({"id":id,"method":"mcpServer/elicitation/request","params":{"mode":"form","requestedSchema":{"type":"object","properties":{}}}});
            assert_eq!(classify(&raw), Inbound::Request);
            let r = request(&raw).unwrap();
            assert_eq!(r.kind, "mcp_confirmation");
            for action in ["accept", "decline", "cancel"] {
                let wire = response(&raw, &r, &json!({"action":action,"content":{}})).unwrap();
                assert_eq!(wire["id"], id);
                assert_eq!(wire["result"]["action"], action);
                assert!(wire["result"].get("decision").is_none());
            }
        }
        assert_eq!(classify(&json!({"id":0,"result":{}})), Inbound::Response);
        for v in [
            json!({"id":null,"method":"x"}),
            json!({"id":[],"method":"x"}),
            json!({"id":1,"result":{},"error":{}}),
            json!({"id":1,"method":"x","result":{}}),
        ] {
            assert_eq!(classify(&v), Inbound::Malformed);
        }
    }
    #[test]
    fn typed_values_bounds_enums_and_unsupported_constraints() {
        let schema = json!({"type":"object","properties":{"text":{"type":"string","minLength":1},"n":{"type":"integer","minimum":1,"maximum":3},"b":{"type":"boolean"},"many":{"type":"array","items":{"anyOf":[{"const":"a","title":"甲"},{"const":"b","title":"乙"}]},"minItems":1}},"required":["n","b"]});
        assert!(validate(&schema, &json!({"text":"中","n":2,"b":false,"many":["a"]})).is_ok());
        for content in [
            json!({"n":"2","b":false}),
            json!({"n":4,"b":false}),
            json!({"n":2.5,"b":false}),
            json!({"n":2}),
            json!({"n":2,"b":true,"many":["x"]}),
            json!({"n":2,"b":true,"many":["a","a"]}),
            json!({"n":2,"b":true,"extra":"secret"}),
        ] {
            assert!(validate(&schema, &content).is_err());
        }
        let raw = json!({"id":1,"params":{"mode":"openai/userVerification","challenge":"secret"}});
        let r = request(&raw).unwrap();
        assert_eq!(r.kind, "mcp_unsupported");
        assert!(!r.detail.contains("secret"));
        assert!(response(&raw, &r, &json!({"action":"accept"})).is_err());
        assert!(response(&raw, &r, &json!({"action":"cancel"})).is_ok());
        assert!(supported(
            &json!({"type":"object","properties":{"x":{"type":"string","pattern":".*"}}})
        )
        .is_err());
    }
}
