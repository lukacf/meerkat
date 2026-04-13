//! Compatibility-only machine schemas retained while generated runtime kernels
//! still depend on absorbed flow/frame/loop surfaces.
//!
//! These are intentionally excluded from the canonical two-kernel catalog.

mod flow_frame;
mod flow_run;
mod loop_iteration;

pub use flow_frame::flow_frame_machine;
pub use flow_run::flow_run_machine;
pub use loop_iteration::loop_iteration_machine;
