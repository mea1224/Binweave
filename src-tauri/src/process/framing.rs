use tokio::io::{AsyncBufRead, AsyncBufReadExt};

// CLI frames can contain MCP catalogs, tool results and resumed thread history.
pub const AGENT_FRAME_LIMIT: usize = 32 * 1024 * 1024;

// fill_buf/consume bounds allocation before a newline arrives. read_line with
// a size check afterwards would allocate without bound on malformed output.
pub async fn next_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Option<String>, String> {
    let mut bytes = Vec::new();
    next_line_buffered(reader, limit, &mut bytes).await
}

// The caller owns partial bytes, so select! cancellation cannot discard a frame.
pub async fn next_line_buffered<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
    bytes: &mut Vec<u8>,
) -> Result<Option<String>, String> {
    loop {
        let available = reader
            .fill_buf()
            .await
            .map_err(|_| "Unable to read process stream")?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }
        let newline = available.iter().position(|b| *b == b'\n');
        let count = newline.map_or(available.len(), |index| index + 1);
        if bytes.len() + count > limit {
            return Err(format!(
                "进程单帧超过大小限制（上限 {limit} 字节，已接收至少 {} 字节）",
                bytes.len() + count
            ));
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            break;
        }
    }
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    Ok(Some(
        String::from_utf8_lossy(&std::mem::take(bytes)).into_owned(),
    ))
}

// Each pipe drains independently, with bounded backpressure. Dropping the owner
// aborts its task on cancellation, including descendants holding an open pipe.
pub struct Pipe {
    receiver: tokio::sync::mpsc::Receiver<Result<String, String>>,
    task: tokio::task::JoinHandle<()>,
}
impl Pipe {
    pub fn new(
        reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
        limit: usize,
        diagnostic: bool,
    ) -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(4);
        let task = tokio::spawn(async move {
            let mut reader = tokio::io::BufReader::new(reader);
            let mut partial = Vec::new();
            loop {
                let result = if diagnostic {
                    match tokio::time::timeout(
                        std::time::Duration::from_millis(20),
                        next_line_buffered(&mut reader, limit, &mut partial),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(_) => {
                            // Do not split a valid UTF-8 code point at the timeout.
                            let end = match std::str::from_utf8(&partial) {
                                Ok(_) => partial.len(),
                                Err(e) if e.error_len().is_none() => e.valid_up_to(),
                                Err(_) => partial.len(),
                            };
                            if end == 0 {
                                continue;
                            }
                            let text = String::from_utf8_lossy(&partial[..end]).into_owned();
                            partial.drain(..end);
                            Ok(Some(text))
                        }
                    }
                } else {
                    next_line_buffered(&mut reader, limit, &mut partial).await
                };
                match result {
                    Ok(Some(line)) => {
                        if sender.send(Ok(line)).await.is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        let _ = sender.send(Err(error)).await;
                        break;
                    }
                }
            }
        });
        Self { receiver, task }
    }
    pub async fn next(&mut self) -> Result<Option<String>, String> {
        self.receiver.recv().await.transpose()
    }
}
impl Drop for Pipe {
    fn drop(&mut self) {
        self.task.abort();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncWriteExt, BufReader};
    #[tokio::test]
    async fn diagnostic_pipe_flushes_without_newline_and_preserves_split_unicode() {
        let (read, mut write) = tokio::io::duplex(64);
        let mut pipe = Pipe::new(read, 1024, true);
        write.write_all(b"partial \xe4").await.unwrap();
        let text = tokio::time::timeout(std::time::Duration::from_secs(1), pipe.next())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(text.as_deref(), Some("partial "));
        write.write_all(b"\xb8\xad\xff\n").await.unwrap();
        assert_eq!(pipe.next().await.unwrap().as_deref(), Some("中�"));
        drop(write);
        assert_eq!(pipe.next().await.unwrap(), None);
    }

    #[tokio::test]
    async fn stalled_stdout_does_not_hold_diagnostics_and_bad_bytes_do_not_abort() {
        let (read, mut write) = tokio::io::duplex(64);
        let mut stdout = Pipe::new(read, 1024, false);
        write.write_all(b"{\"unfinished\":").await.unwrap();
        let (error_read, mut error_write) = tokio::io::duplex(64);
        let mut stderr = Pipe::new(error_read, 1024, true);
        error_write.write_all(b"bad \xff\n").await.unwrap();
        assert_eq!(stderr.next().await.unwrap().as_deref(), Some("bad �"));
        write.write_all(b"true}\n").await.unwrap();
        assert_eq!(
            stdout.next().await.unwrap().as_deref(),
            Some("{\"unfinished\":true}")
        );
    }
    #[tokio::test]
    async fn delivers_complete_frame_without_waiting_for_pipe_close_or_buffer_fill() {
        let (read, mut write) = tokio::io::duplex(8192);
        write.write_all(b"delta\n").await.unwrap();
        let mut reader = BufReader::new(read);
        let line = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            next_line(&mut reader, 1024),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(line.as_deref(), Some("delta"));
        // Writer remains open: no EOF or full buffer is necessary to receive a frame.
        write.write_all(b"next\n").await.unwrap();
        assert_eq!(
            next_line(&mut reader, 1024).await.unwrap().as_deref(),
            Some("next")
        );
    }
    #[tokio::test]
    async fn bounds_unterminated_lines_before_allocating_more() {
        let bytes = vec![b'x'; 2048];
        assert!(next_line(&mut &bytes[..], 1024).await.is_err());
    }
    #[tokio::test]
    async fn cancelled_read_retains_partial_frame() {
        let (read, mut write) = tokio::io::duplex(64);
        let mut reader = BufReader::new(read);
        let mut partial = Vec::new();
        write.write_all(b"partial ").await.unwrap();
        assert!(tokio::time::timeout(
            std::time::Duration::from_millis(10),
            next_line_buffered(&mut reader, 100, &mut partial)
        )
        .await
        .is_err());
        write.write_all(b"frame\n").await.unwrap();
        assert_eq!(
            next_line_buffered(&mut reader, 100, &mut partial)
                .await
                .unwrap()
                .as_deref(),
            Some("partial frame")
        );
    }
    #[tokio::test]
    async fn preserves_utf8_across_chunks_and_final_partial_line() {
        let (read, mut write) = tokio::io::duplex(8);
        let task = tokio::spawn(async move {
            for b in "raw café 中文\r\nlast".as_bytes() {
                write.write_all(&[*b]).await.unwrap();
            }
        });
        let mut reader = BufReader::with_capacity(2, read);
        assert_eq!(
            next_line(&mut reader, 100).await.unwrap().as_deref(),
            Some("raw café 中文")
        );
        assert_eq!(
            next_line(&mut reader, 100).await.unwrap().as_deref(),
            Some("last")
        );
        assert_eq!(next_line(&mut reader, 100).await.unwrap(), None);
        task.await.unwrap();
    }
}
