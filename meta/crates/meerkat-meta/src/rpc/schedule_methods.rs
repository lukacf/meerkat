//! `meta/schedule/tools`: the definitions of Meerkat's durable schedule tools,
//! exactly as Meerkat's own `schedule/tools` lists them.
//!
//! The result is Meerkat's `ScheduleToolsResult`, built from
//! `meerkat::schedule_tools_list()`: the `meerkat_schedule_*` tools a member
//! whose profile sets `tools.schedule = true` is given. It is static. The
//! answer opens no schedule store, starts no schedule host and reads no
//! runtime state, and it ignores params, as Meerkat's `schedule/tools` does.
//!
//! These are the base definitions. Inside a member's session Meerkat also lets
//! `meerkat_schedule_create` and `meerkat_schedule_update` take a session
//! target of `type: "current_session"`, which the tool host resolves to a
//! durable target before it persists the schedule. Whether a gateway attached
//! the schedule tools at all is a separate fact: the `schedule` slot of the
//! `storage` summary in `meta/status` and `meta/capabilities`.
//!
//! Serve or omit, per surface:
//! - unified stdin RPC (`handle_unified_rpc_json_inner`): served and
//!   advertised;
//! - console runtime RPC (`handle_console_runtime_rpc_with_visibility`): served
//!   and advertised, as a read method that needs no grant;
//! - module-only stdin RPC (`handle_meta_rpc_json`): omitted, because the
//!   module runtime has no mob and no members;
//! - console aggregator RPC: omitted, because it has no single runtime the
//!   definitions would describe;
//! - Flow Editor RPC: omitted, because it serves mobpack authoring only.

/// The method name.
pub(crate) const SCHEDULE_TOOLS_METHOD: &str = "meta/schedule/tools";

/// The `meta/schedule/tools` result: Meerkat's `ScheduleToolsResult`.
pub(crate) fn schedule_tools_result() -> serde_json::Value {
    serde_json::json!({ "tools": meerkat::schedule_tools_list() })
}

#[cfg(test)]
mod tests {
    use super::schedule_tools_result;

    #[test]
    fn result_is_the_core_schedule_tools_wire_result() -> Result<(), serde_json::Error> {
        let result: meerkat_contracts::wire::ScheduleToolsResult =
            serde_json::from_value(schedule_tools_result())?;
        assert_eq!(result.tools, meerkat::schedule_tools_list());
        Ok(())
    }
}
