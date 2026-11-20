use serde_json::{json, Map, Value};

pub enum Plan {
    Call(Value),
    Pending(&'static str, &'static str),
}

// Context is local to one diagnostic connection. Never recover arguments from
// arbitrary prose, schema examples or a different server's session.
#[derive(Default)]
pub struct Context {
    document: Option<Value>,
    procedure: Option<Value>,
    address: Option<Value>,
    text: Option<Value>,
    path: Option<Value>,
    target_open: Option<bool>,
    ambiguous: bool,
}

pub fn priority(name: &str) -> u8 {
    match name {
        "current_document" => 0,
        "list_documents" => 1,
        "binary_session" => 2,
        "list_procedures" => 3,
        "current_procedure" => 4,
        "procedure_address" | "procedure_info" => 4,
        "list_strings" => 5,
        _ => 6,
    }
}

fn safe(tool: &Value) -> Result<(), (&'static str, &'static str)> {
    let name = tool["name"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if tool["annotations"]["destructiveHint"] == true {
        return Err(("approval_required", "服务端标记为可能破坏状态，需审批。"));
    }
    let words: Vec<_> = name.split('_').collect();
    if words.iter().any(|w| {
        matches!(
            *w,
            "set"
                | "unset"
                | "write"
                | "rename"
                | "delete"
                | "remove"
                | "add"
                | "append"
                | "create"
                | "update"
                | "patch"
                | "annotate"
                | "comment"
                | "start"
                | "stop"
                | "close"
                | "open"
                | "attach"
                | "detach"
                | "resume"
                | "kill"
                | "launch"
                | "execute"
                | "exec"
                | "command"
                | "load"
                | "unload"
                | "install"
                | "uninstall"
                | "push"
                | "pull"
                | "reset"
                | "record"
                | "export"
                | "save"
                | "capture"
                | "act"
                | "modify"
                | "edit"
                | "clear"
                | "overwrite"
                | "replace"
                | "run"
                | "invoke"
                | "eval"
                | "debugger"
                | "inject"
                | "restart"
                | "step"
                | "continue"
        )
    }) && !matches!(
        name.as_str(),
        "inspect_native_load_image"
            | "inspect_pe_load_config"
            | "inspect_with_objdump"
            | "inspect_packet_capture"
            | "inspect_pcap_capture"
            | "inspect_web_network_capture"
    ) {
        return Err((
            "approval_required",
            "涉及状态修改、写入或进程／设备控制，需审批，未自动执行。",
        ));
    }
    // Match reviewed query families and subjects, not arbitrary verbs hiding
    // behind a query-looking prefix (e.g. procedure_retype or get_or_create).
    let query = name
        .split_once('_')
        .is_some_and(|(verb, subject)| match verb {
            "procedure" => matches!(
                subject,
                "address"
                    | "assembly"
                    | "callees"
                    | "callers"
                    | "info"
                    | "pseudo_code"
                    | "references"
            ),
            "current" => matches!(subject, "address" | "document" | "procedure"),
            "get" => matches!(
                subject,
                "call_graph"
                    | "objc_classes"
                    | "objc_protocols"
                    | "navigation_context"
                    | "symbols"
                    | "strings"
                    | "structures"
            ),
            "read" => matches!(subject, "bytes" | "function_instructions"),
            "list" => matches!(
                subject,
                "documents"
                    | "procedures"
                    | "strings"
                    | "names"
                    | "segments"
                    | "architectures"
                    | "bookmarks"
                    | "symbols"
                    | "structures"
                    | "functions"
                    | "jeb_units"
            ),
            "search" => matches!(
                subject,
                "procedures" | "strings" | "bytes" | "symbols" | "functions" | "android_classes"
            ),
            "inspect" => matches!(
                subject,
                "signature"
                    | "native_instruction"
                    | "native_load_image"
                    | "native_data_type"
                    | "native_api"
                    | "native_dispatch_metadata"
                    | "address_context"
                    | "analysis_settings"
                    | "binary_layout"
                    | "native_exports"
                    | "pe_imports"
                    | "pe_resources"
                    | "pe_load_config"
                    | "pe_certificates"
                    | "elf_notes"
                    | "elf_unwind_ranges"
                    | "macho"
                    | "native_initializers"
                    | "binary_hardening"
                    | "go_binary"
                    | "rust_binary"
                    | "symbol"
                    | "structure"
                    | "with_objdump"
            ),
            _ => false,
        })
        || matches!(
            name.as_str(),
            "xrefs"
                | "binary_overview"
                | "binary_session"
                | "decompile_jeb_item"
                | "decompile"
                | "disasm"
                | "address_name"
                | "address_to_file_offset"
                | "resolve_containing_procedure"
                | "find_xrefs_to_name"
        );
    if query || tool["annotations"]["readOnlyHint"] == true {
        Ok(())
    } else {
        Err((
            "unsupported_rule",
            "尚无此工具的自动验证规则，未判定为不可用。",
        ))
    }
}

pub fn plan(tool: &Value, context: &Context) -> Plan {
    if let Err((outcome, detail)) = safe(tool) {
        return Plan::Pending(outcome, detail);
    }
    let schema = &tool["inputSchema"];
    let Some(fields) = schema.as_object() else {
        return Plan::Pending("unsupported_rule", "参数 schema 无效，无法构造合法调用。");
    };
    if schema["type"] != "object"
        || fields.keys().any(|k| {
            !matches!(
                k.as_str(),
                "type"
                    | "properties"
                    | "required"
                    | "additionalProperties"
                    | "description"
                    | "title"
                    | "$schema"
                    | "examples"
                    | "$defs"
            )
        })
        || schema
            .get("required")
            .is_some_and(|v| !v.as_array().is_some_and(|a| a.iter().all(Value::is_string)))
    {
        return Plan::Pending("unsupported_rule", "参数包含暂不支持的约束，尚未执行验证。");
    }
    let empty = Map::new();
    let properties = schema["properties"].as_object().unwrap_or(&empty);
    let required = schema["required"].as_array().cloned().unwrap_or_default();
    let name = tool["name"].as_str().unwrap_or_default();
    if context.target_open == Some(false)
        && context.document.is_none()
        && (properties.contains_key("document") || name == "binary_overview")
    {
        return Plan::Pending(
            "needs_context",
            "当前诊断连接尚无打开的目标，请准备目标后重新诊断。",
        );
    }
    if context.ambiguous
        && (properties.contains_key("document") || matches!(name, "binary_overview" | "xrefs"))
    {
        return Plan::Pending(
            "needs_context",
            "存在多个文档且没有当前目标，请先选择目标后重新诊断。",
        );
    }
    let mut arguments = Map::new();
    for (key, field) in properties {
        let candidate = match key.as_str() {
            "document" | "document_name" => context.document.clone(),
            "procedure" | "function" | "function_name" => context.procedure.clone(),
            "address" | "ea" => context.address.clone(),
            "path" | "file_path" | "target_path" => context.path.clone(),
            "pattern" | "query" if name == "search_strings" => context.text.clone(),
            "pattern" | "query" if name == "search_procedures" => context.procedure.clone(),
            "length" | "size" | "limit" | "count" | "max_results" => {
                let minimum = field["minimum"].as_u64().unwrap_or(1).max(1);
                if minimum > 16 {
                    return Plan::Pending(
                        "unsupported_rule",
                        "schema 要求的最小读取量超过诊断限额。",
                    );
                }
                Some(json!(minimum))
            }
            "refresh_candidates" if name == "binary_session" => Some(json!(false)),
            "saveSnapshot" | "save_snapshot" | "write_back" | "persist" => Some(json!(false)),
            // These are known read-only modes, not arbitrary enum/default values.
            "mode" if matches!(name, "search_strings" | "search_procedures") => {
                Some(json!("literal"))
            }
            _ => None,
        };
        if let Some(value) = candidate {
            match valid(field, &value) {
                Some(true) => {
                    arguments.insert(key.clone(), value);
                }
                _ if required.contains(&json!(key)) => {
                    return Plan::Pending(
                        "unsupported_rule",
                        "已知上下文不符合参数约束，尚未执行验证。",
                    )
                }
                _ => {
                    return Plan::Pending(
                        "unsupported_rule",
                        "已知上下文或安全限额不符合参数约束，尚未执行验证。",
                    )
                }
            }
        }
    }
    if required
        .iter()
        .any(|key| !arguments.contains_key(key.as_str().unwrap_or_default()))
    {
        return Plan::Pending(
            "needs_context",
            "需目标／业务参数待测；尚无符合 schema 的真实上下文。",
        );
    }
    Plan::Call(Value::Object(arguments))
}

// Return None for constraints we cannot validate; never silently ignore them.
fn valid(schema: &Value, value: &Value) -> Option<bool> {
    let fields = schema.as_object()?;
    if fields.keys().any(|k| {
        !matches!(
            k.as_str(),
            "type"
                | "description"
                | "title"
                | "default"
                | "examples"
                | "enum"
                | "const"
                | "minimum"
                | "maximum"
                | "minLength"
                | "maxLength"
        )
    }) {
        return None;
    }
    let typed = match schema["type"].as_str()? {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        _ => return None,
    };
    Some(
        typed
            && schema.get("const").is_none_or(|v| v == value)
            && schema
                .get("enum")
                .is_none_or(|v| v.as_array().is_some_and(|a| a.contains(value)))
            && value.as_str().is_none_or(|s| {
                let len = s.chars().count() as u64;
                schema["minLength"].as_u64().is_none_or(|n| len >= n)
                    && schema["maxLength"].as_u64().is_none_or(|n| len <= n)
            })
            && value.as_f64().is_none_or(|n| {
                schema["minimum"].as_f64().is_none_or(|min| n >= min)
                    && schema["maximum"].as_f64().is_none_or(|max| n <= max)
            }),
    )
}

fn identity(value: &Value, keys: &[&str]) -> Option<Value> {
    if value
        .as_str()
        .is_some_and(|s| !s.is_empty() && s.len() <= 4096)
    {
        return Some(value.clone());
    }
    keys.iter().find_map(|key| {
        value
            .get(key)
            .filter(|v| v.is_string())
            .and_then(|v| identity(v, &[]))
    })
}

fn payloads(result: &Value) -> Vec<Value> {
    if let Some(value) = result.get("structuredContent") {
        return vec![value.clone()];
    }
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v["text"].as_str())
        .filter_map(|s| serde_json::from_str(s).ok())
        .collect()
}

impl Context {
    pub fn observe(&mut self, name: &str, result: &Value) {
        for payload in payloads(result) {
            let value = payload
                .get("raw_result")
                .filter(|v| !v.is_null())
                .or_else(|| payload.get("normalized_result").filter(|v| !v.is_null()))
                .or_else(|| payload.get("result"))
                .unwrap_or(&payload);
            match name {
                "binary_session" => {
                    self.target_open = value["open"].as_bool();
                    if self.target_open == Some(true) {
                        self.path = identity(&value["path"], &[]);
                    }
                }
                "current_document" => {
                    self.document = identity(value, &["document", "name"]);
                    self.ambiguous = false;
                }
                "list_documents" if self.document.is_none() => {
                    if let Some(items) = value.as_array().or_else(|| value["documents"].as_array())
                    {
                        self.ambiguous = items.len() > 1;
                        if items.len() == 1 {
                            self.document = identity(&items[0], &["document", "name"]);
                        }
                    }
                }
                "list_procedures" | "current_procedure"
                    if !self.ambiguous && self.procedure.is_none() =>
                {
                    let first = value
                        .as_array()
                        .or_else(|| value["procedures"].as_array())
                        .and_then(|a| a.first())
                        .unwrap_or(value);
                    self.procedure =
                        identity(first, &["name", "procedure", "address", "entrypoint"]);
                    self.address = first
                        .get("address")
                        .or_else(|| first.get("entrypoint"))
                        .and_then(|v| identity(v, &[]));
                }
                "procedure_address" if !self.ambiguous => {
                    self.address = identity(value, &["address", "entrypoint"]);
                }
                "procedure_info" if !self.ambiguous => {
                    if let Some(address) = identity(&value["entrypoint"], &[]) {
                        self.address = Some(address);
                    }
                }
                "list_strings" if !self.ambiguous => {
                    let first = value
                        .as_array()
                        .or_else(|| value["strings"].as_array())
                        .and_then(|a| a.first());
                    if let Some(first) = first {
                        self.text = identity(first, &["text", "string", "value"]);
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn failure(result: &Value) -> (&'static str, &'static str) {
    // Only inspect error payloads; output fixed descriptions, never server text.
    let text = result.to_string().to_ascii_lowercase();
    if [
        "no active target",
        "no target",
        "no document",
        "target_not_open",
        "target_required",
        "target_unavailable",
        "no binary",
        "no app is open",
        "no selected",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        (
            "needs_context",
            "调用提示缺少当前目标或文档，请准备目标后重新诊断。",
        )
    } else if [
        "capability_unavailable",
        "provider_unavailable",
        "not supported",
        "unsupported",
        "not installed",
        "enoent",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        ("unavailable", "调用明确报告能力不支持或环境依赖不可用。")
    } else {
        ("failed", "工具返回执行错误；未保存服务端错误正文。")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_state_and_path_are_scoped_to_this_connection() {
        let mut context = Context::default();
        let tool = json!({"name":"binary_overview","inputSchema":{"type":"object"}});
        context.observe(
            "binary_session",
            &json!({"structuredContent":{"result":{"open":false}}}),
        );
        assert!(matches!(
            plan(&tool, &context),
            Plan::Pending("needs_context", _)
        ));
        context.observe(
            "binary_session",
            &json!({"structuredContent":{"result":{"open":true,"path":"D:/fixture.exe"}}}),
        );
        let file_tool = json!({"name":"inspect_binary_layout","inputSchema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}});
        let Plan::Call(args) = plan(&file_tool, &context) else {
            panic!("missing target path")
        };
        assert_eq!(args["path"], "D:/fixture.exe");
        assert!(matches!(
            plan(&file_tool, &Context::default()),
            Plan::Pending("needs_context", _)
        ));
        assert_eq!(
            failure(&json!({"message":"No app is open."})).0,
            "needs_context"
        );
        assert!(safe(&json!({"name":"procedure_retype"})).is_err());
    }
    #[test]
    fn semantic_queries_override_missing_or_false_annotations_but_not_mutations() {
        for name in [
            "list_documents",
            "procedure_pseudo_code",
            "inspect_signature",
            "get_call_graph",
            "xrefs",
        ] {
            assert!(safe(&json!({"name":name,"annotations":{"readOnlyHint":false}})).is_ok());
        }
        for name in [
            "procedure_rename",
            "get_and_delete",
            "close_binary",
            "set_comment",
            "rizin_debug_command",
        ] {
            assert_eq!(
                safe(&json!({"name":name,"annotations":{"readOnlyHint":true}}))
                    .unwrap_err()
                    .0,
                "approval_required"
            );
        }
    }
    #[test]
    fn uses_observed_identity_and_bounds_reads_without_guessing_examples() {
        let mut context = Context::default();
        let tool = json!({"name":"read_bytes","inputSchema":{"type":"object","properties":{"address":{"type":"string"},"length":{"type":"integer","minimum":1,"default":256}},"required":["address"],"examples":[{"address":"0x1000"}]}});
        assert!(matches!(
            plan(&tool, &context),
            Plan::Pending("needs_context", _)
        ));
        context.observe(
            "procedure_address",
            &json!({"structuredContent":{"raw_result":"0x140001234"}}),
        );
        let Plan::Call(args) = plan(&tool, &context) else {
            panic!("missing call")
        };
        assert_eq!(args, json!({"address":"0x140001234","length":1}));
    }
    #[test]
    fn ambiguous_documents_and_unsupported_constraints_stay_pending() {
        let mut context = Context::default();
        context.observe(
            "list_documents",
            &json!({"structuredContent":{"raw_result":["a","b"]}}),
        );
        let tool = json!({"name":"list_procedures","inputSchema":{"type":"object","properties":{"document":{"type":"string"}}}});
        assert!(matches!(
            plan(&tool, &context),
            Plan::Pending("needs_context", _)
        ));
        context.observe(
            "current_document",
            &json!({"structuredContent":{"raw_result":"b"}}),
        );
        let Plan::Call(args) = plan(&tool, &context) else {
            panic!("missing call")
        };
        assert_eq!(args["document"], "b");
        assert_eq!(
            valid(&json!({"type":"string","pattern":"a"}), &json!("b")),
            None
        );
        assert_eq!(
            valid(&json!({"type":"integer","minimum":5}), &json!(1)),
            Some(false)
        );
    }
}
