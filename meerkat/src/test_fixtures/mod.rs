//! Explicitly-featured test-support fixtures (never production
//! composition). Gated by `test-realtime-fixtures` (ADJ-P6B-4): the
//! deterministic realtime fakes are shared by the facade live-pipeline
//! battery, `meerkat-mob/tests` (which cannot depend on the integration
//! crate — cycle), and the integration member-live rows.

pub mod realtime;
