use super::{
    framing, hidden_command,
    parser::{self, ParsedOutput},
    terminate,
};
use crate::model::AgentId;
use std::{collections::HashMap, path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWriteExt},
    sync::oneshot,
};

pub struct RunSpec {
    pub interactive: Option<super::interactive::Config>,
    pub agent: AgentId,
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: HashMap<String, String>,
    pub input: Option<String>,
    pub deadline: Duration,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunError {
    Cancelled,
    TimedOut,
    ThinkingTimedOut,
    Failed(String),
}
impl From<String> for RunError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}

async fn pump<R: AsyncRead + Unpin + Send + 'static, F: Fn(ParsedOutput) -> Result<(), String>>(
    reader: R,
    agent: AgentId,
    stderr: bool,
    output: &F,
) -> Result<(), String> {
    let mut reader = framing::Pipe::new(reader, framing::AGENT_FRAME_LIMIT, stderr);
    let mut total = 0usize;
    let mut count = 0usize;
    while let Some(line) = reader.next().await? {
        total += line.len();
        count += 1;
        if total > 16 * 1024 * 1024 || count > 100_000 {
            return Err("Worker output exceeds the per-turn safety limit".into());
        }
        // Include empty lines and unknown JSON verbatim; only newline delimiters
        // are removed. The callback must persist before publishing UI events.
        output(parser::parse(agent, &line, stderr))?;
        tokio::task::yield_now().await;
    }
    Ok(())
}

// Fixed CLI adapters authorize turns; this is never exposed as arbitrary IPC.
pub async fn run<F: Fn(ParsedOutput) -> Result<(), String>>(
    spec: &RunSpec,
    mut cancel: oneshot::Receiver<()>,
    output: F,
) -> Result<i32, RunError> {
    if let Some(config) = &spec.interactive {
        return super::interactive::run(spec, config, cancel, output).await;
    }
    let watchdog = super::thinking_timeout::Watchdog::new(super::thinking_timeout::LIMIT);
    let output = |event: ParsedOutput| {
        if super::thinking_timeout::is_progress(spec.agent, &event) {
            watchdog.progress();
        }
        output(event)
    };
    if !matches!(cancel.try_recv(), Err(oneshot::error::TryRecvError::Empty)) {
        return Err(RunError::Cancelled);
    }
    if !spec.cwd.is_dir() {
        return Err(RunError::Failed("Worker directory does not exist".into()));
    }
    let mut command = hidden_command(&spec.executable)?;
    super::supervision::configure(&mut command);
    command
        .args(&spec.args)
        .envs(&spec.env)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|_| RunError::Failed("Unable to launch worker executable".into()))?;
    let tree = match super::supervision::Tree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            terminate(&mut child).await;
            return Err(error.into());
        }
    };
    let input = child
        .stdin
        .take()
        .ok_or_else(|| RunError::Failed("Worker stdin unavailable".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RunError::Failed("Worker stdout unavailable".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| RunError::Failed("Worker stderr unavailable".into()))?;
    let result = {
        let work = async {
            let write = async {
                let mut input = input;
                if let Some(text) = &spec.input {
                    input
                        .write_all(text.as_bytes())
                        .await
                        .map_err(|_| "Unable to write worker input".to_owned())?;
                }
                input
                    .shutdown()
                    .await
                    .map_err(|_| "Unable to close worker stdin".to_owned())?;
                Ok::<_, String>(())
            };
            let (_, _, _, status) = tokio::try_join!(
                write,
                pump(stdout, spec.agent, false, &output),
                pump(stderr, spec.agent, true, &output),
                async {
                    child
                        .wait()
                        .await
                        .map_err(|_| "Unable to wait for worker".to_owned())
                }
            )?;
            Ok::<_, RunError>(status.code().unwrap_or(-1))
        };
        tokio::pin!(work);
        tokio::select! {
            biased;
            _ = &mut cancel => Err(RunError::Cancelled),
            _ = tokio::time::sleep(spec.deadline) => Err(RunError::TimedOut),
            _ = watchdog.expired() => Err(RunError::ThinkingTimedOut),
            result = &mut work => result,
        }
    };
    drop(tree);
    if result.is_err() {
        terminate(&mut child).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, sync::Mutex};
    #[test]
    #[ignore = "Subprocess fixture, invoked by streaming tests"]
    fn child_fixture() {
        if std::env::var("BINWEAVE_FIXTURE_MODE").as_deref() == Ok("deltas") {
            for _ in 0..120 {
                println!(
                    "{}",
                    r#"{"method":"item/agentMessage/delta","params":{"itemId":"message","delta":"中"}}"#
                );
            }
            std::io::stdout().flush().unwrap();
            // The timer must deliver the tail while the child is still alive.
            std::thread::sleep(Duration::from_millis(250));
            println!("{}", r#"{"type":"turn.completed"}"#);
            return;
        }
        println!("{{\"type\":\"thread.started\",\"thread_id\":\"fixture-session\"}}");
        eprintln!("fixture stderr");
        std::io::stdout().flush().unwrap();
        if std::env::var("BINWEAVE_FIXTURE_MODE").as_deref() == Ok("stall") {
            std::thread::sleep(Duration::from_secs(20));
        }
        print!("last partial line");
        std::io::stdout().flush().unwrap();
    }
    fn fixture(stall: bool) -> RunSpec {
        RunSpec {
            interactive: None,
            agent: AgentId::Codex,
            executable: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            args: vec![
                "--exact".into(),
                "process::stream::tests::child_fixture".into(),
                "--ignored".into(),
                "--nocapture".into(),
            ],
            cwd: std::env::temp_dir(),
            env: HashMap::from([(
                "BINWEAVE_FIXTURE_MODE".into(),
                if stall { "stall" } else { "exit" }.into(),
            )]),
            input: None,
            deadline: Duration::from_secs(5),
        }
    }
    #[tokio::test]
    async fn microbatches_deliver_live_tail_and_flush_before_terminal_event() {
        let mut spec = fixture(false);
        spec.env
            .insert("BINWEAVE_FIXTURE_MODE".into(), "deltas".into());
        let (_sender, receiver) = oneshot::channel();
        let batches = Mutex::new(Vec::new());
        super::super::batching::run(&spec, receiver, |events| {
            batches
                .lock()
                .unwrap()
                .push((std::time::Instant::now(), events));
            Ok(())
        })
        .await
        .unwrap();
        let batches = batches.into_inner().unwrap();
        let texts: Vec<_> = batches
            .iter()
            .filter(|(_, events)| events.iter().any(|e| e.summary.is_some()))
            .collect();
        assert!(
            texts.len() < 60,
            "120 deltas must not produce 120 publications"
        );
        assert_eq!(
            texts
                .iter()
                .flat_map(|(_, events)| events.iter())
                .filter_map(|e| e.summary.as_deref())
                .collect::<String>(),
            "中".repeat(120)
        );
        let terminal = batches
            .iter()
            .find(|(_, events)| events.iter().any(|e| e.raw.contains("turn.completed")))
            .unwrap();
        assert!(
            terminal.0.duration_since(texts[0].0) >= Duration::from_millis(100),
            "text must be delivered before child completion"
        );
    }
    #[tokio::test]
    async fn captures_both_pipes_without_a_shell() {
        let (sender, receiver) = oneshot::channel();
        let lines = Mutex::new(Vec::new());
        assert_eq!(
            run(&fixture(false), receiver, |line| {
                lines.lock().unwrap().push(line);
                Ok(())
            })
            .await
            .unwrap(),
            0
        );
        drop(sender);
        let lines = lines.lock().unwrap();
        assert!(lines
            .iter()
            .any(|line| line.session_id.as_deref() == Some("fixture-session")));
        assert!(lines.iter().any(|line| line.raw == "fixture stderr"));
        assert!(lines
            .iter()
            .any(|line| line.raw.contains("last partial line")));
    }
    #[tokio::test]
    async fn cancels_and_times_out_stalled_children() {
        let mut spec = fixture(true);
        spec.deadline = Duration::from_millis(100);
        let (_sender, receiver) = oneshot::channel();
        assert_eq!(
            run(&spec, receiver, |_| Ok(())).await,
            Err(RunError::TimedOut)
        );
        spec.deadline = Duration::from_secs(5);
        let (sender, receiver) = oneshot::channel();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = sender.send(());
        });
        assert_eq!(
            run(&spec, receiver, |_| Ok(())).await,
            Err(RunError::Cancelled)
        );
    }
}
