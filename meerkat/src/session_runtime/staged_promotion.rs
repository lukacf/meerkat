//! Staged-session promotion lifecycle.
//!
//! Populated by W1-B (`PendingPromotionCleanup`) and W2-D
//! (staged-session lifecycle helpers — `spawn_pending_create_*`,
//! `await_service_apply_runtime_turn`,
//! `finish_pending_promotion_after_service_turn`, etc.).
