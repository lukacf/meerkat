//! Stock SDK gateway. Application lifecycle owners are composed by their host binary.

fn main() {
    // SAFETY: the first statement of main: no other thread exists yet and
    // nothing has used SQLite, as the switch requires.
    #[allow(unsafe_code)]
    let _ = unsafe { meerkat_sqlite::disable_memory_statistics_at_process_start() };
    meerkat_meta::rpc_gateway::main();
}
