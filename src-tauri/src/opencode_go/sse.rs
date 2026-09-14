use super::*;

impl SseDecoder {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, String> {
        if self.buffer.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err("provider response is too large".to_string());
        }
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some((separator, delimiter_len)) = sse_separator(&self.buffer) {
            let frame = self.buffer[..separator].to_vec();
            self.buffer.drain(..separator + delimiter_len);
            let frame = std::str::from_utf8(&frame)
                .map_err(|_| "provider stream is not UTF-8".to_string())?;
            let mut event = None;
            let mut data = Vec::new();
            for line in frame.lines() {
                let line = line.trim_end_matches('\r');
                if let Some(value) = line.strip_prefix("event:") {
                    event = Some(value.trim().to_string());
                } else if let Some(value) = line.strip_prefix("data:") {
                    data.push(value.trim_start());
                }
            }
            if !data.is_empty() {
                events.push(SseEvent {
                    event,
                    data: data.join("\n"),
                });
            }
        }
        Ok(events)
    }

    pub(crate) fn finish(&self) -> Result<(), String> {
        if self.buffer.iter().all(u8::is_ascii_whitespace) {
            Ok(())
        } else {
            Err("provider stream ended with an incomplete event".to_string())
        }
    }
}

pub(super) fn sse_separator(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => Some((left, 2)),
        (Some(_), Some(right)) => Some((right, 4)),
        (Some(left), None) => Some((left, 2)),
        (None, Some(right)) => Some((right, 4)),
        (None, None) => None,
    }
}
