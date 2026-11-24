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

/// The refusal of a mob beside the host's default mob over a console log
/// store that does not read one mob's frames itself
/// (`ConsoleLogStore::reads_one_mob`): such a store serves the default mob
/// alone. The mob's console events are not registered over it, and a read
/// of the mob's timeline is answered this refusal, never a page.
///
/// It travels as a log error: it is the error of the refused registration,
/// and for the read it is the boxed error of
/// [`ConsoleTimelineQueryError::Operational`], where
/// [`ConsoleTimelineQueryError::store_serves_the_default_mob_alone`] tells
/// it from a store's fault. The published error type gains no variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoreServesTheDefaultMobAlone {
    /// The mob that has no console log in the store.
    pub(crate) mob: meerkat_mob::MobId,
}

impl std::fmt::Display for StoreServesTheDefaultMobAlone {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "this console log store does not read one mob's frames \
             (ConsoleLogStore::reads_one_mob): it serves the host's default mob alone, and the \
             mob `{}` has no console log in it",
            self.mob.as_str()
        )
    }
}

impl std::error::Error for StoreServesTheDefaultMobAlone {}

/// The refusal of an operation that names a mob beside the host's default
/// mob, by a console log store that keeps the trait's provided form of it
/// (such as [`super::ConsoleLogStore::update_frame_status_of_mob`]): that form
/// reaches the default mob's frames alone. It is boxed into the store's
/// error and told apart from a store's fault by its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationServesTheDefaultMobAlone {
    /// The mob the operation named.
    pub(crate) mob: meerkat_mob::MobId,
    /// What the operation does, in words.
    pub(crate) operation: &'static str,
}

impl std::fmt::Display for OperationServesTheDefaultMobAlone {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "this console log store does not {} for a mob beside the host's default mob: it \
             keeps the provided form of the operation, and the mob `{}` is not reached by it",
            self.operation,
            self.mob.as_str()
        )
    }
}

impl std::error::Error for OperationServesTheDefaultMobAlone {}

impl ConsoleTimelineQueryError {
    /// This error as the refusal of a mob beside the default mob over a
    /// store that reads no one mob's frames
    /// ([`StoreServesTheDefaultMobAlone`]), when it is that refusal: a door
    /// answers it as what it is, apart from a store's fault.
    pub(crate) fn store_serves_the_default_mob_alone(
        &self,
    ) -> Option<&StoreServesTheDefaultMobAlone> {
        match self {
            Self::Operational(error) => error.downcast_ref(),
            _ => None,
        }
    }
}
