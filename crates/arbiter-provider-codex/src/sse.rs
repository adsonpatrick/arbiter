use arbiter_core::events::TokenUsage;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalResponseMetadata {
    pub response_id: String,
    pub usage: TokenUsage,
}

#[derive(Default)]
pub struct SseMetadataParser {
    pending: Vec<u8>,
}

impl std::fmt::Debug for SseMetadataParser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SseMetadataParser")
            .field("pending_bytes", &self.pending.len())
            .finish()
    }
}

impl SseMetadataParser {
    pub fn push(&mut self, chunk: &[u8]) -> Option<TerminalResponseMetadata> {
        self.pending.extend_from_slice(chunk);
        let mut terminal = None;

        while let Some(boundary) = event_boundary(&self.pending) {
            let event = self.pending.drain(..boundary).collect::<Vec<_>>();
            let delimiter_len = if self.pending.starts_with(b"\r\n\r\n") {
                4
            } else {
                2
            };
            self.pending.drain(..delimiter_len);

            terminal = parse_terminal_event(&event).or(terminal);
        }

        terminal
    }
}

fn event_boundary(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .or_else(|| bytes.windows(2).position(|window| window == b"\n\n"))
}

fn parse_terminal_event(event: &[u8]) -> Option<TerminalResponseMetadata> {
    let event = std::str::from_utf8(event).ok()?;
    let data = event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    let value: serde_json::Value = serde_json::from_str(&data).ok()?;
    if value.get("type")?.as_str()? != "response.completed" {
        return None;
    }

    let response = value.get("response")?;
    let usage = response.get("usage")?;
    Some(TerminalResponseMetadata {
        response_id: response.get("id")?.as_str()?.to_owned(),
        usage: TokenUsage {
            input_tokens: usage.get("input_tokens")?.as_u64()?,
            cached_input_tokens: usage
                .pointer("/input_tokens_details/cached_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
            output_tokens: usage.get("output_tokens")?.as_u64()?,
            reasoning_tokens: usage
                .pointer("/output_tokens_details/reasoning_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::SseMetadataParser;

    const COMPLETED_RESPONSE: &[u8] =
        include_bytes!("../../../tests/fixtures/response_completed.sse");

    #[test]
    fn parses_terminal_response_metadata() {
        let mut parser = SseMetadataParser::default();

        let metadata = parser.push(COMPLETED_RESPONSE).expect("terminal metadata");

        assert_eq!(metadata.response_id, "resp_arbiter_m0");
        assert_eq!(metadata.usage.input_tokens, 120);
        assert_eq!(metadata.usage.cached_input_tokens, 32);
        assert_eq!(metadata.usage.output_tokens, 24);
        assert_eq!(metadata.usage.reasoning_tokens, 8);
    }

    #[test]
    fn parses_terminal_metadata_at_every_chunk_boundary() {
        for split in 0..=COMPLETED_RESPONSE.len() {
            let mut parser = SseMetadataParser::default();
            let first = parser.push(&COMPLETED_RESPONSE[..split]);
            let second = parser.push(&COMPLETED_RESPONSE[split..]);
            let metadata = second
                .or(first)
                .unwrap_or_else(|| panic!("terminal metadata missing at byte split {split}"));

            assert_eq!(metadata.response_id, "resp_arbiter_m0");
            assert_eq!(metadata.usage.reasoning_tokens, 8);
        }
    }

    #[test]
    fn parser_debug_never_exposes_buffered_response_content() {
        let mut parser = SseMetadataParser::default();
        parser.push(b"data: raw-response-secret-sentinel");

        assert_eq!(
            format!("{parser:?}"),
            "SseMetadataParser { pending_bytes: 34 }"
        );
    }
}
