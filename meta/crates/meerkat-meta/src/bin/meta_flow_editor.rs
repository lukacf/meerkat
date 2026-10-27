use std::net::SocketAddr;

use meerkat_meta::{flow_editor_router, flow_editor_router_with_host_deploy};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The worker count follows meerkat's `RKAT_WORKER_THREADS`, then
    // `TOKIO_WORKER_THREADS`, as for the gateways; unset, Tokio starts one
    // worker per available CPU.
    let mut runtime = tokio::runtime::Builder::new_multi_thread();
    runtime.enable_all();
    if let Some(workers) = meerkat_runtime::host_stack::worker_threads_from_env()? {
        runtime.worker_threads(workers.get());
    }
    runtime.build()?.block_on(serve())
}

async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    // Predecessor environment variables would be silently ignored.
    meerkat_meta::storage_predecessor::refuse_predecessor_environment()?;
    let options = server_options()?;
    let listen = options.listen;
    let listener = tokio::net::TcpListener::bind(listen).await?;
    let addr = listener.local_addr()?;
    // Callers may pass `--listen 127.0.0.1:0` and read the bound address
    // from this line (see console/fixture-ready.cjs).
    println!("META_FIXTURE_READY {{\"addr\":\"{addr}\"}}");
    if options.allow_host_deploy {
        eprintln!("meta flow editor listening on http://{addr}/flow-editor (host deploy enabled)");
        // The host-deploy surface is UNAUTHENTICATED and runs `rkat mob run` on
        // this host. On a loopback bind that is contained to local users, but a
        // non-loopback bind exposes unauthenticated host code execution to the
        // network. Warn loudly so an operator cannot do this unknowingly.
        if !addr.ip().is_loopback() {
            eprintln!(
                "WARNING: host deploy is enabled on a NON-LOOPBACK address ({addr}). This \
                 exposes an UNAUTHENTICATED surface that runs `rkat mob run` on this host to \
                 anyone who can reach {addr} — i.e. network-reachable remote code execution. \
                 Bind to 127.0.0.1 (the default) unless you have placed an authenticating proxy \
                 in front of this listener."
            );
        }
        axum::serve(listener, flow_editor_router_with_host_deploy()).await?;
    } else {
        eprintln!("meta flow editor listening on http://{addr}/flow-editor");
        axum::serve(listener, flow_editor_router()).await?;
    }
    Ok(())
}

struct ServerOptions {
    listen: SocketAddr,
    allow_host_deploy: bool,
}

fn server_options() -> Result<ServerOptions, Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut listen = None;
    let mut allow_host_deploy = env_flag("META_FLOW_EDITOR_ALLOW_HOST_DEPLOY");
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => {
                let value = args.next().ok_or("--listen requires HOST:PORT")?;
                listen = Some(value.parse()?);
            }
            "--allow-host-deploy" => {
                allow_host_deploy = true;
            }
            "--help" | "-h" => {
                eprintln!("Usage: meta_flow_editor [--listen HOST:PORT] [--allow-host-deploy]");
                std::process::exit(0);
            }
            other => {
                return Err(format!("unknown argument: {other}").into());
            }
        }
    }
    let listen = match listen {
        Some(value) => value,
        None => std::env::var("META_FLOW_EDITOR_LISTEN_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:4191".to_string())
            .parse()?,
    };
    Ok(ServerOptions {
        listen,
        allow_host_deploy,
    })
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}
