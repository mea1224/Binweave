//! Conservative approval gate, not a shell parser. Unknown operations fail closed.
use crate::model::AgentId;
use serde_json::Value;
use std::path::{Component, Path};

pub fn scoped_file(root: &Path, name: &str) -> bool {
    if name.is_empty() || name.contains(['\n', '\r', '*', '?']) {
        return false;
    }
    let path = Path::new(name);
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return false;
    }
    let lexical_root = root.to_path_buf();
    let Ok(root) = root.canonicalize() else {
        return false;
    };
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let Ok(relative) = path
        .strip_prefix(&root)
        .or_else(|_| path.strip_prefix(&lexical_root))
    else {
        return false;
    };
    let mut current = root.clone();
    for component in relative.components() {
        current.push(component);
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            return false;
        };
        if metadata.file_type().is_symlink() {
            return false;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return false;
            }
        }
    }
    path.is_file() && path.canonicalize().is_ok_and(|p| p.starts_with(root))
}

fn single_file_command(command: &str, root: &Path) -> bool {
    // No shell composition, substitutions, flags, globs, scripts or redirects.
    if command.contains([
        ';', '|', '&', '>', '<', '$', '`', '\n', '\r', '(', ')', '!', '%',
    ]) {
        return false;
    }
    let operand = command
        .strip_prefix("cat ")
        .or_else(|| command.strip_prefix("Get-Content -LiteralPath "));
    let Some(operand) = operand.map(str::trim) else {
        return false;
    };
    let path = if operand.len() >= 2 && operand.starts_with('\'') && operand.ends_with('\'') {
        &operand[1..operand.len() - 1]
    } else if operand.contains(char::is_whitespace) {
        return false;
    } else {
        operand
    };
    !path.starts_with('-') && !path.contains(['\'', '"', ':']) && scoped_file(root, path)
}

pub fn may_ask(agent: AgentId, value: &Value, root: &Path) -> bool {
    if agent == AgentId::Codex {
        let p = &value["params"];
        return value["method"] == "item/commandExecution/requestApproval"
            && p["additionalPermissions"].is_null()
            && p["networkApprovalContext"].is_null()
            && p["cwd"]
                .as_str()
                .is_some_and(|cwd| Path::new(cwd).canonicalize().ok() == root.canonicalize().ok())
            && p["command"]
                .as_str()
                .is_some_and(|s| single_file_command(s, root));
    }
    let request = &value["request"];
    let input = &request["input"];
    match request["tool_name"].as_str().unwrap_or("") {
        "Read" => input["file_path"]
            .as_str()
            .is_some_and(|s| scoped_file(root, s)),
        "Bash" if input["dangerouslyDisableSandbox"] != true => input["command"]
            .as_str()
            .is_some_and(|s| single_file_command(s, root)),
        // MCP server tools keep their own authorization; never whitelist arbitrary MCPs
        // merely because the name contains 'read'. Only the configured REA namespace
        // and semantic read tools observed in the REA tools/list probe can reach
        // human approval here. An approval is never a permanent tool grant.
        tool => {
            let parts: Vec<_> = tool.split("__").collect();
            parts.len() == 3
                && parts[0] == "mcp"
                && parts[1].eq_ignore_ascii_case("rea")
                && matches!(
                    parts[2],
                    "decompile_jeb_item"
                        | "read_function_instructions"
                        | "inspect_native_instruction"
                        | "inspect_signature"
                        | "find_xrefs_to_name"
                        | "xrefs"
                        | "list_strings"
                        | "search_strings"
                        | "read_bytes"
                )
                && !input.to_string().contains("..")
                && input.as_object().is_some_and(|fields| {
                    fields.iter().all(|(key, value)| {
                        if key.contains("path") || key.contains("file") || key.contains("directory")
                        {
                            value.as_str().is_some_and(|s| scoped_file(root, s))
                        } else {
                            value.is_string()
                                || value.is_number()
                                || value.is_boolean()
                                || value.is_null()
                        }
                    })
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn reads_are_scoped_and_shell_composition_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("sample.bin"), b"bytes").unwrap();
        for command in ["cat sample.bin", "Get-Content -LiteralPath 'sample.bin'"] {
            assert!(may_ask(
                AgentId::Claude,
                &json!({"request":{"tool_name":"Bash","input":{"command":command}}}),
                root.path()
            ));
            assert!(may_ask(
                AgentId::Codex,
                &json!({"method":"item/commandExecution/requestApproval","params":{"cwd":root.path(),"command":command}}),
                root.path()
            ));
        }
        for command in [
            "cat ../sample.bin",
            "cat sample.bin; whoami",
            "cat $(pwd)",
            "cat sample.bin > output",
            "cat --help",
            "python read.py",
            "cat sample.bin sample.bin",
            "cat %SECRET%",
        ] {
            assert!(!single_file_command(command, root.path()), "{command}");
        }
        assert!(!scoped_file(root.path(), "../outside"));
        assert!(scoped_file(
            root.path(),
            &root.path().join("sample.bin").to_string_lossy()
        ));
        assert!(!may_ask(
            AgentId::Claude,
            &json!({"request":{"tool_name":"mcp__rea__execute_script","input":{"code":"anything"}}}),
            root.path()
        ));
        assert!(may_ask(
            AgentId::Claude,
            &json!({"request":{"tool_name":"mcp__rea__read_function_instructions","input":{"address":"0x123"}}}),
            root.path()
        ));
        assert!(!may_ask(
            AgentId::Claude,
            &json!({"request":{"tool_name":"mcp__rea__read_bytes","input":{"file_path":"../outside"}}}),
            root.path()
        ));
        assert!(!may_ask(
            AgentId::Claude,
            &json!({"request":{"tool_name":"Write","input":{"file_path":"sample.bin"}}}),
            root.path()
        ));
        assert!(!may_ask(
            AgentId::Codex,
            &json!({"method":"item/permissions/requestApproval","params":{"permissions":{"network":true}}}),
            root.path()
        ));
    }
}
