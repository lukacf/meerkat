use super::{ConsoleCursor, ConsoleLogError};

/// Query failures preserve the distinction between missing replay and a broken read.
/// Custom stores can return a boxed instance through `ConsoleLogResult`.
#[derive(Debug)]
pub enum ConsoleTimelineQueryError {
    ReplayUnavailable {
        requested_cursor: Option<ConsoleCursor>,
        latest_cursor: Option<ConsoleCursor>,
    },
    Operational(ConsoleLogError),
    PaginationNoProgress,
}

impl std::fmt::Display for ConsoleTimelineQueryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReplayUnavailable { .. } => formatter
                .write_str("timeline replay cursor is unavailable at the current store frontier"),
            Self::Operational(error) => write!(formatter, "timeline query failed: {error}"),
            Self::PaginationNoProgress => {
                formatter.write_str("timeline replay made no cursor progress")
            }
        }
    }
}

impl std::error::Error for ConsoleTimelineQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Operational(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<ConsoleLogError> for ConsoleTimelineQueryError {
    fn from(error: ConsoleLogError) -> Self {
        match error.downcast::<Self>() {
            Ok(typed) => *typed,
            Err(error) => Self::Operational(error),
        }
    }
}

pub type ConsoleTimelineQueryResult<T> = Result<T, ConsoleTimelineQueryError>;
