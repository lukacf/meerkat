#![allow(
    clippy::collapsible_if,
    clippy::double_ended_iterator_last,
    clippy::format_collect,
    clippy::if_not_else,
    clippy::manual_clamp,
    clippy::self_only_used_in_recursion,
    clippy::semicolon_if_nothing_returned,
    clippy::too_many_arguments,
    clippy::uninlined_format_args,
    clippy::useless_conversion,
    clippy::useless_format
)]

#[cfg(not(test))]
mod artifacts;
mod render;

#[cfg(not(test))]
pub use artifacts::{
    composition_route_coverage_operator_name, composition_scheduler_coverage_operator_name,
    composition_witness_cfg_name, render_composition_ci_cfg, render_composition_contract_markdown,
    render_composition_semantic_model, render_composition_witness_cfg, render_machine_ci_cfg,
    render_machine_contract_markdown, render_machine_semantic_model,
};
pub use render::render_machine_module;
#[cfg(not(test))]
pub use render::{
    GENERATED_COVERAGE_END, GENERATED_COVERAGE_START, merge_mapping_document,
    render_composition_mapping_coverage, render_composition_module, render_generated_kernel_mod,
    render_machine_kernel_module, render_machine_mapping_coverage,
};
