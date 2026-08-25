use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_CORRELATION: AtomicU64 = AtomicU64::new(1);
const MAX_KIND_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ToOpenAi,
    FromOpenAi,
}

impl Direction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ToOpenAi => "to_openai",
            Self::FromOpenAi => "from_openai",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalClass {
    Codec,
    Http,
    WebSocket,
    Closed,
}

impl TerminalClass {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Codec => "codec",
            Self::Http => "http",
            Self::WebSocket => "websocket",
            Self::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireSummary {
    pub direction: Direction,
    pub kind: String,
    pub byte_count: usize,
    pub local_correlation: u64,
    pub terminal_class: Option<TerminalClass>,
}

impl WireSummary {
    #[must_use]
    pub fn event(direction: Direction, kind: &str, byte_count: usize) -> Self {
        Self {
            direction,
            kind: sanitize_kind(kind),
            byte_count,
            local_correlation: NEXT_CORRELATION.fetch_add(1, Ordering::Relaxed),
            terminal_class: None,
        }
    }

    #[must_use]
    pub fn terminal(
        direction: Direction,
        kind: &str,
        byte_count: usize,
        terminal_class: TerminalClass,
    ) -> Self {
        Self {
            terminal_class: Some(terminal_class),
            ..Self::event(direction, kind, byte_count)
        }
    }

    pub fn emit(&self) {
        let terminal_class = self.terminal_class.map(TerminalClass::as_str);
        tracing::debug!(
            target: "oai_rt_rs::experimental::gpt_live",
            direction = self.direction.as_str(),
            kind = self.kind,
            byte_count = self.byte_count,
            local_correlation = self.local_correlation,
            terminal_class,
            "private realtime wire event"
        );
    }
}

fn sanitize_kind(kind: &str) -> String {
    if kind.is_empty()
        || kind.len() > MAX_KIND_BYTES
        || !kind
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return "unknown".to_owned();
    }
    kind.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsafe_kind_is_not_loggable() {
        let summary = WireSummary::event(Direction::FromOpenAi, "token\nsecret", 12);
        assert_eq!(summary.kind, "unknown");
    }
}
