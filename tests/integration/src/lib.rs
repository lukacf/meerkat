#![allow(clippy::bool_assert_comparison)]

pub mod coverage_matrix;
pub mod e2e_lanes;
#[cfg(not(target_arch = "wasm32"))]
pub mod fixtures;
