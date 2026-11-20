mod config;
mod protocol;
mod strategy;

use crate::{
    model::{EnvCheck, EnvCheckReport},
    process,
};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncReadExt, BufReader},
    time::timeout,
};

pub async fn inspect_environment(workspace: &Path) -> EnvCheckReport {
    let (codex, claude) = tokio::join!(
        check_cli("codex", "Codex CLI"),
        check_cli("claude", "Claude Code CLI")
    );
    let (mut checks, entries) = config::discover(workspace);
    checks.splice(0..0, [codex, claude]);
    let mut mcp_servers = Vec::new();
    for (mut report, spec) in entries {
        if let Some(spec) = spec {
            match probe(&spec).await {
                Ok(checks) => {
                    report.connection_status = "ok".into();
                    report.tools = checks
                        .iter()
                        .filter(|c| c.status == "ok")
                        .map(|c| c.name.clone())
                        .collect();
                    report.status = if report.tools.is_empty() {
                        "warning"
                    } else {
                        "ok"
                    }
                    .into();
                    report.detail = format!(
                        "MCP 握手成功；实际调用验证可用 {} 个工具，另有 {} 个工具未验证可用。",
                        report.tools.len(),
                        checks.len() - report.tools.len()
                    );
                    report.tool_checks = checks;
                }
                Err(error) => {
                    report.connection_status = "error".into();
                    report.status = "error".into();
                    report.detail = error;
                }
            }
        }
        mcp_servers.push(report);
    }
    let rea_path = process::resolve("rea")
        .or_else(|_| process::resolve("rea-agents"))
        .ok();
    let reachable = mcp_servers.iter().any(|s| s.status == "ok");
    checks.push(EnvCheck {
        id: "rea".into(),
        label: "REA availability".into(),
        status: if reachable { "ok" } else { "warning" }.into(),
        detail: if reachable {
            "REA is available through a verified MCP server; a PATH alias is not required."
        } else if rea_path.is_some() {
            "REA command found. MCP communication is not verified."
        } else {
            "No REA executable on PATH and no verified MCP connection."
        }
        .into(),
        path: rea_path.map(|p| p.to_string_lossy().into_owned()),
    });
    EnvCheckReport {
        checked_at: chrono::Utc::now(),
        platform: std::env::consts::OS.into(),
        checks,
        mcp_servers,
    }
}
async fn check_cli(name: &str, label: &str) -> EnvCheck {
    let path = process::resolve(name)
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    let (status, detail) =
        match process::capture(name, &["--version"], Duration::from_secs(8)).await {
            Ok(version) => (
                "ok",
                format!(
                    "Version check passed: {}. Authentication and model access are not verified.",
                    version
                        .lines()
                        .next()
                        .unwrap_or("unknown")
                        .chars()
                        .take(160)
                        .collect::<String>()
                ),
            ),
            Err(error) => ("warning", error),
        };
    EnvCheck {
        id: name.into(),
        label: label.into(),
        status: status.into(),
        detail,
        path,
    }
}
async fn probe(spec: &config::ProbeSpec) -> Result<Vec<crate::model::McpToolCheck>, String> {
    let mut command = process::hidden_command(&spec.command)?;
    command
        .args(&spec.args)
        .envs(&spec.env)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| "Unable to launch configured MCP executable")?;
    let mut input = child.stdin.take().ok_or("MCP stdin is unavailable")?;
    let output = child.stdout.take().ok_or("MCP stdout is unavailable")?;
    let stderr = child.stderr.take().ok_or("MCP stderr is unavailable")?;
    // Drain logs without storing potentially sensitive environment values.
    let drain = tokio::spawn(async move {
        let mut reader = stderr.take(process::OUTPUT_LIMIT);
        let _ = tokio::io::copy(&mut reader, &mut tokio::io::sink()).await;
    });
    let mut reader = BufReader::new(output);
    let result = timeout(
        Duration::from_secs(10),
        protocol::handshake(&mut reader, &mut input),
    )
    .await
    .map_err(|_| "MCP handshake timed out after 10 seconds".to_string())
    .and_then(|value| value);
    let result = match result {
        Ok(tools) => Ok(protocol::verify_tools(&mut reader, &mut input, tools).await),
        Err(error) => Err(error),
    };
    drop(input);
    process::terminate(&mut child).await;
    drain.abort();
    let _ = drain.await;
    result
}
