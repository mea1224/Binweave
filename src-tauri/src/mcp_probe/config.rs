use crate::model::{AgentId, EnvCheck, McpServerReport};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub struct ProbeSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: PathBuf,
}
type Entry = (McpServerReport, Option<ProbeSpec>);

pub fn discover(workspace: &Path) -> (Vec<EnvCheck>, Vec<Entry>) {
    let mut checks = Vec::new();
    let mut entries = Vec::new();
    let home = dirs::home_dir().unwrap_or_else(|| workspace.to_path_buf());
    let codex_home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let claude_home = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let claude_file = if std::env::var_os("CLAUDE_CONFIG_DIR").is_some() {
        claude_home.join(".claude.json")
    } else {
        home.join(".claude.json")
    };
    let files = [
        (codex_home.join("config.toml"), AgentId::Codex, true),
        (claude_file, AgentId::Claude, true),
        (workspace.join(".mcp.json"), AgentId::Claude, false),
        (workspace.join(".codex/config.toml"), AgentId::Codex, false),
    ];
    for (path, owner, trusted) in files {
        if !trusted && !path.exists() {
            continue;
        }
        let parsed = read_config(&path);
        let (status, detail) = match &parsed {
            Ok(_) => (
                "ok",
                "Configuration parsed. Credentials are never included in the report.".to_owned(),
            ),
            Err(error) => (
                if path.exists() {
                    "error"
                } else {
                    "not_checked"
                },
                error.clone(),
            ),
        };
        checks.push(EnvCheck {
            id: format!("config:{}", path.display()),
            label: format!("{:?} MCP configuration", owner),
            status: status.into(),
            detail,
            path: Some(path.to_string_lossy().into_owned()),
        });
        let Ok(data) = parsed else {
            continue;
        };
        let key = if owner == AgentId::Codex {
            "mcp_servers"
        } else {
            "mcpServers"
        };
        entries.extend(parse_servers(
            data.get(key),
            owner,
            &path,
            workspace,
            trusted,
            "global",
        ));
        if owner == AgentId::Claude {
            if let Some(projects) = data.get("projects").and_then(Value::as_object) {
                for (project, value) in projects {
                    if same_path(Path::new(project), workspace) {
                        entries.extend(parse_servers(
                            value.get("mcpServers"),
                            owner,
                            &path,
                            workspace,
                            false,
                            "project",
                        ));
                    }
                }
            }
        }
    }
    for owner in [AgentId::Codex, AgentId::Claude] {
        if !entries.iter().any(|(r, _)| r.owner == owner) {
            entries.push((
                McpServerReport {
                    id: format!("{owner:?}-rea-missing"),
                    name: format!("REA ({owner:?})"),
                    owner,
                    transport: "unknown".into(),
                    source: "No matching entry".into(),
                    status: "not_checked".into(),
                    detail: "No REA MCP entry found for this agent.".into(),
                    tools: Vec::new(),
                    connection_status: "not_checked".into(),
                    tool_checks: Vec::new(),
                },
                None,
            ));
        }
    }
    (checks, entries)
}
fn same_path(a: &Path, b: &Path) -> bool {
    let a = a
        .canonicalize()
        .unwrap_or_else(|_| a.to_owned())
        .to_string_lossy()
        .into_owned();
    let b = b
        .canonicalize()
        .unwrap_or_else(|_| b.to_owned())
        .to_string_lossy()
        .into_owned();
    if cfg!(windows) {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}
fn read_config(path: &Path) -> Result<Value, String> {
    let metadata =
        std::fs::metadata(path).map_err(|_| "Configuration is unavailable or missing")?;
    if metadata.len() > 8 * 1024 * 1024 {
        return Err("Configuration exceeds the 8 MiB safety limit".into());
    }
    let text = std::fs::read_to_string(path).map_err(|_| "Unable to read configuration")?;
    let text = text.trim_start_matches('\u{feff}');
    if path.extension().and_then(|s| s.to_str()) == Some("toml") {
        let data: toml::Value =
            toml::from_str(text).map_err(|_| "Invalid TOML configuration (contents omitted)")?;
        serde_json::to_value(data).map_err(|_| "Unable to parse TOML configuration".into())
    } else {
        serde_json::from_str(text)
            .map_err(|_| "Invalid JSON configuration (contents omitted)".into())
    }
}
fn is_rea(name: &str, value: &Value) -> bool {
    let words: Vec<String> = name
        .to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_owned)
        .collect();
    if words.iter().any(|s| s == "rea") {
        return true;
    }
    std::iter::once(value.get("command").and_then(Value::as_str).unwrap_or(""))
        .chain(
            value
                .get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
        .any(|s| {
            let s = s.to_ascii_lowercase();
            s.contains("rea-agents")
                || s.contains("reverse-engineer-anything")
                || s.ends_with("rea.mjs")
        })
}
fn parse_servers(
    value: Option<&Value>,
    owner: AgentId,
    source: &Path,
    workspace: &Path,
    trusted: bool,
    scope: &str,
) -> Vec<Entry> {
    let Some(servers) = value.and_then(Value::as_object) else {
        return Vec::new();
    };
    servers.iter().filter(|(name, value)| is_rea(name, value)).take(16).map(|(name, entry)| {
        let transport = entry.get("type").and_then(Value::as_str).unwrap_or(if entry.get("url").is_some() { "http" } else { "stdio" });
        let mut report = McpServerReport { id: format!("{owner:?}:{}:{scope}:{name}", source.display()), name: format!("{name} ({owner:?}, {scope})"), owner, transport: transport.into(), source: source.to_string_lossy().into_owned(), status: "not_checked".into(), connection_status: "not_checked".into(), detail: String::new(), tools: Vec::new(), tool_checks: Vec::new() };
        if entry.get("enabled").and_then(Value::as_bool) == Some(false) || entry.get("disabled").and_then(Value::as_bool) == Some(true) { report.detail = "Server is disabled in configuration.".into(); return (report, None); }
        if transport != "stdio" { report.detail = "Configured remote transport detected. HTTP/SSE probing is not implemented in this foundation release.".into(); return (report, None); }
        if !trusted { report.detail = "Project MCP command detected but not executed. Explicit project trust is required.".into(); return (report, None); }
        let spec = parse_spec(entry, workspace);
        match spec {
            Ok(spec) => { report.detail = "Configured stdio server; probing protocol.".into(); (report, Some(spec)) },
            Err(error) => { report.status = "error".into(); report.detail = error; (report, None) }
        }
    }).collect()
}
fn parse_spec(value: &Value, workspace: &Path) -> Result<ProbeSpec, String> {
    let command = value["command"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("MCP command is missing")?
        .to_owned();
    let args: Vec<String> =
        serde_json::from_value(value.get("args").cloned().unwrap_or(serde_json::json!([])))
            .map_err(|_| "MCP args must be an array of strings")?;
    let env: HashMap<String, String> =
        serde_json::from_value(value.get("env").cloned().unwrap_or(serde_json::json!({})))
            .map_err(|_| "MCP environment values must be strings")?;
    if command.contains("${") || args.iter().chain(env.values()).any(|v| v.contains("${")) {
        return Err("Unresolved environment substitution in MCP configuration".into());
    }
    let cwd = value
        .get("cwd")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .map(|p| {
            if p.is_absolute() {
                p
            } else {
                workspace.join(p)
            }
        })
        .unwrap_or_else(|| workspace.into());
    if !cwd.is_dir() {
        return Err("Configured MCP working directory does not exist".into());
    }
    Ok(ProbeSpec {
        command,
        args,
        env,
        cwd,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn matches_rea_without_false_positive_on_great() {
        assert!(is_rea("rea", &json!({})));
        assert!(!is_rea("great-server", &json!({})));
        assert!(is_rea("analysis", &json!({"args":["/scripts/rea.mjs"]})));
    }
    #[test]
    fn never_launches_untrusted_project_commands() {
        let data = json!({"rea":{"command":"malicious.exe"}});
        let entries = parse_servers(
            Some(&data),
            AgentId::Claude,
            Path::new(".mcp.json"),
            Path::new("."),
            false,
            "project",
        );
        assert!(entries[0].1.is_none());
        assert!(entries[0].0.detail.contains("trust"));
    }
    #[test]
    fn secrets_do_not_enter_reports() {
        let dir = tempfile::tempdir().unwrap();
        let data = json!({"rea":{"command":"node","env":{"TOKEN":"secret-value"}}});
        let entries = parse_servers(
            Some(&data),
            AgentId::Codex,
            Path::new("config.toml"),
            dir.path(),
            true,
            "global",
        );
        assert_eq!(entries[0].1.as_ref().unwrap().env["TOKEN"], "secret-value");
        assert!(!serde_json::to_string(&entries[0].0)
            .unwrap()
            .contains("secret-value"));
    }
    #[test]
    fn accepts_quoted_toml_names_and_reports_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[mcp_servers.\"rea\"]\ncommand = 'node'\n").unwrap();
        assert_eq!(
            read_config(&path).unwrap()["mcp_servers"]["rea"]["command"],
            "node"
        );
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "secret-value").unwrap();
        assert!(!read_config(&path).unwrap_err().contains("secret"));
    }
}
