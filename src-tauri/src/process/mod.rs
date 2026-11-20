pub mod batching;
mod elicitation;
pub mod framing;
pub mod interactive;
pub mod parser;
mod read_policy;
pub mod stream;
pub mod supervision;
pub mod thinking_timeout;

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::{Child, Command},
    time::timeout,
};

pub const CREATE_NO_WINDOW: u32 = 0x08000000;
pub const OUTPUT_LIMIT: u64 = 1024 * 1024;

// Known npm shims are resolved to their real entry point. Never interpolate a
// user prompt or configuration argument into cmd.exe / PowerShell source.
pub fn hidden_command(program: &str) -> Result<Command, String> {
    let path = resolve(program)?;
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut command = if cfg!(windows) && matches!(extension.as_str(), "cmd" | "bat" | "ps1") {
        let root = path.parent().ok_or("CLI shim has no parent directory")?;
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let candidates = match name {
            "codex" => vec![root.join("node_modules/@openai/codex/bin/codex.js")],
            "claude" => vec![
                root.join("node_modules/@anthropic-ai/claude-code/bin/claude.exe"),
                root.join("node_modules/@anthropic-ai/claude-code/cli.js"),
            ],
            "rea" | "rea-agents" => vec![root.join("node_modules/rea-agents/scripts/rea.mjs")],
            _ => vec![],
        };
        let entry = candidates.into_iter().find(|p| p.is_file()).ok_or("Unsupported Windows shell shim. Configure the native executable or Node script explicitly.")?;
        script_or_native(&entry)?
    } else {
        script_or_native(&path)?
    };
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command.kill_on_drop(true);
    Ok(command)
}
fn script_or_native(path: &Path) -> Result<Command, String> {
    if matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("js" | "mjs" | "cjs")
    ) {
        let mut command = Command::new(resolve("node")?);
        command.arg(path);
        Ok(command)
    } else {
        Ok(Command::new(path))
    }
}
pub fn resolve(program: &str) -> Result<PathBuf, String> {
    which::which(program).map_err(|_| "Executable not found or not executable".into())
}
async fn bounded_read<R: AsyncRead + Unpin>(reader: R) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader
        .take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| "Unable to read process output")?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        return Err("Process output exceeded the diagnostic limit".into());
    }
    Ok(bytes)
}
pub async fn terminate(child: &mut Child) {
    if timeout(Duration::from_millis(250), child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
        let _ = timeout(Duration::from_secs(2), child.wait()).await;
    }
}
pub async fn capture(program: &str, args: &[&str], duration: Duration) -> Result<String, String> {
    let mut command = hidden_command(program)?;
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|_| "Unable to start executable")?;
    let stdout = child.stdout.take().ok_or("Missing stdout pipe")?;
    let stderr = child.stderr.take().ok_or("Missing stderr pipe")?;
    let result = timeout(duration, async {
        let (out, _err, status) =
            tokio::try_join!(bounded_read(stdout), bounded_read(stderr), async {
                child
                    .wait()
                    .await
                    .map_err(|_| "Unable to wait for process".to_string())
            })?;
        if !status.success() {
            return Err("Executable returned a failure status".into());
        }
        Ok(String::from_utf8_lossy(&out).trim().to_owned())
    })
    .await
    .map_err(|_| "Executable timed out".to_string())
    .and_then(|result| result);
    if result.is_err() {
        terminate(&mut child).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_window_flag_is_correct() {
        assert_eq!(CREATE_NO_WINDOW, 0x08000000);
    }
    #[tokio::test]
    async fn reads_and_limits_output() {
        assert_eq!(bounded_read(&b"hello"[..]).await.unwrap(), b"hello");
        let bytes = vec![b'x'; OUTPUT_LIMIT as usize + 1];
        assert!(bounded_read(bytes.as_slice()).await.is_err());
    }
    #[test]
    fn rejects_missing_executable() {
        assert!(hidden_command("binweave-nonexistent-executable-0").is_err());
    }
}
