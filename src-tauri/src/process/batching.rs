use super::{
    parser::ParsedOutput,
    stream::{self, RunError, RunSpec},
};
use std::{sync::Mutex, time::Duration};
use tokio::sync::oneshot;

pub const WINDOW: Duration = Duration::from_millis(20);

// Only text deltas may wait. Tools, approvals, session IDs, errors and terminal
// frames flush the preceding text first, retaining the original audit order.
fn is_delta(output: &ParsedOutput) -> bool {
    if output.interaction.is_some()
        || output.resolved_interaction.is_some()
        || output.reported_failure
        || output.kind == crate::model::EventKind::Stderr
    {
        return false;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&output.raw) else {
        return false;
    };
    matches!(
        value["method"].as_str(),
        Some(
            "item/agentMessage/delta"
                | "item/reasoning/textDelta"
                | "item/reasoning/summaryTextDelta"
                | "item/commandExecution/outputDelta"
                | "item/fileChange/outputDelta"
        )
    ) || (value["type"] == "stream_event" && value["event"]["type"] == "content_block_delta")
}

#[derive(Default)]
struct Batch {
    outputs: Vec<ParsedOutput>,
    bytes: usize,
}
impl Batch {
    fn push(&mut self, output: ParsedOutput) -> bool {
        let immediate = !is_delta(&output);
        self.bytes += output.raw.len();
        self.outputs.push(output);
        immediate || self.outputs.len() >= 256 || self.bytes >= 256 * 1024
    }
    fn flush(
        &mut self,
        output: &impl Fn(Vec<ParsedOutput>) -> Result<(), String>,
    ) -> Result<(), String> {
        if self.outputs.is_empty() {
            return Ok(());
        }
        self.bytes = 0;
        output(std::mem::take(&mut self.outputs))
    }
}

pub async fn run(
    spec: &RunSpec,
    mut cancel: oneshot::Receiver<()>,
    output: impl Fn(Vec<ParsedOutput>) -> Result<(), String>,
) -> Result<i32, RunError> {
    let batch = Mutex::new(Batch::default());
    let (stop, stopped) = oneshot::channel();
    let work = stream::run(spec, stopped, |event| {
        let mut batch = batch.lock().map_err(|_| "流式缓冲锁异常")?;
        if batch.push(event) {
            batch.flush(&output)?;
        }
        Ok(())
    });
    tokio::pin!(work);
    let mut timer = tokio::time::interval(WINDOW);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let result = loop {
        tokio::select! {
            biased;
            _ = &mut cancel => {
                let _ = stop.send(());
                let _ = (&mut work).await;
                break Err(RunError::Cancelled);
            }
            _ = timer.tick() => {
                let saved = batch.lock().unwrap().flush(&output);
                if let Err(error) = saved {
                    let _ = stop.send(());
                    let _ = (&mut work).await;
                    break Err(RunError::Failed(error));
                }
                // Slow disks must not keep the timer permanently ready and
                // starve pipe processing in this biased select.
                timer.reset();
            }
            result = &mut work => break result,
        }
    };
    batch.lock().unwrap().flush(&output)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::AgentId, process::parser};
    #[test]
    fn batches_deltas_but_flushes_tools_and_bounds_memory() {
        let mut batch = Batch::default();
        for _ in 0..255 {
            assert!(!batch.push(parser::parse(
                AgentId::Codex,
                r#"{"method":"item/agentMessage/delta","params":{"delta":"中"}}"#,
                false
            )));
        }
        assert!(batch.push(parser::parse(AgentId::Claude, r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"文"}}}"#, false)));
        batch
            .flush(&|events| {
                assert_eq!(events.len(), 256);
                Ok(())
            })
            .unwrap();
        assert!(batch.push(parser::parse(
            AgentId::Codex,
            r#"{"method":"item/started"}"#,
            false
        )));
        assert!(batch.push(parser::parse(AgentId::Codex, "error", true)));
    }
}
