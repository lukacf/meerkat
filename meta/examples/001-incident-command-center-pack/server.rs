#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args,
    clippy::collapsible_if,
    clippy::redundant_clone,
    clippy::needless_raw_string_hashes,
    clippy::single_match,
    clippy::redundant_closure_for_method_calls,
    clippy::redundant_pattern_matching,
    clippy::ignored_unit_patterns,
    clippy::clone_on_copy,
    clippy::manual_assert,
    clippy::unwrap_in_result,
    clippy::useless_vec
)]

#[path = "incident_command_center.rs"]
mod incident_command_center;

use incident_command_center::{
    build_runtime_bundle, build_runtime_bundle_with_default_client, incident_image_model,
    incident_model, scenario_path,
};

fn main() {
    // Meerkat 0.7's generated machine-authority apply path allocates very
    // large stack frames in debug builds (see .cargo/config.toml). The
    // workspace-level `[env] RUST_MIN_STACK` only covers `cargo run`/test
    // flows, not direct execution of the prebuilt binary, so the example
    // sizes its own threads explicitly: the root future runs on a dedicated
    // 32 MiB thread and tokio workers get 32 MiB stacks (mirrors
    // meta_gateway/rpc_gateway's explicit worker sizing).
    const STACK_SIZE: usize = 32 * 1024 * 1024;
    std::thread::Builder::new()
        .name("icc-runtime".into())
        .stack_size(STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_stack_size(STACK_SIZE)
                .build()
                .expect("build tokio runtime");
            if let Err(error) = runtime.block_on(run()) {
                eprintln!("incident command center failed: {error:#}");
                std::process::exit(1);
            }
        })
        .expect("spawn runtime thread")
        .join()
        .expect("runtime thread panicked");
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "meerkat_meta=info,axum=info".to_string()),
        )
        .with_target(false)
        .compact()
        .try_init()
        .ok();

    let offline = env_flag("INCIDENT_COMMAND_CENTER_OFFLINE");
    let bundle = if offline {
        println!("incident command center mode=offline-topology");
        Box::pin(build_runtime_bundle_with_default_client(
            &scenario_path()?,
            std::sync::Arc::new(meerkat_client::TestClient::default()),
        ))
        .await?
    } else {
        let _ = std::env::var("OPENAI_API_KEY")
            .map_err(|_| "Set OPENAI_API_KEY to run the live incident command center example")?;
        println!(
            "incident command center model={} image_model={}",
            incident_model(),
            incident_image_model()
        );
        Box::pin(build_runtime_bundle(&scenario_path()?)).await?
    };
    let listen_addr = std::env::var("INCIDENT_COMMAND_CENTER_LISTEN_ADDR")
        .unwrap_or_else(|_| bundle.scenario.listen_addr.clone());
    let listener = tokio::net::TcpListener::bind(&listen_addr).await?;
    let bound = listener.local_addr()?;
    println!("incident command center listening on http://{bound}");
    // Harnesses pass a port-0 address and read the bound one from this line;
    // see console/fixture-ready.cjs.
    println!("META_FIXTURE_READY {{\"addr\":\"{bound}\"}}");
    println!("GET  /console");
    println!("GET  /console/experience");
    println!("POST /console/rpc");
    println!("GET  /console/timeline/stream");
    bundle.runtime.serve(listener, bundle.decisions).await?;
    Ok(())
}

fn env_flag(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}
