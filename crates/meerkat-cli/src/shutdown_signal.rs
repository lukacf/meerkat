#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownSignal {
    Interrupt,
    Terminate,
}

async fn select_shutdown_signal<C, T>(ctrl_c: C, terminate: T) -> std::io::Result<ShutdownSignal>
where
    C: std::future::Future<Output = std::io::Result<()>>,
    T: std::future::Future<Output = std::io::Result<()>>,
{
    tokio::pin!(ctrl_c);
    tokio::pin!(terminate);
    tokio::select! {
        result = &mut ctrl_c => {
            result?;
            Ok(ShutdownSignal::Interrupt)
        }
        result = &mut terminate => {
            result?;
            Ok(ShutdownSignal::Terminate)
        }
    }
}

/// Wait for either interactive interruption or a service-manager shutdown
/// signal before starting graceful process cleanup.
pub(crate) async fn wait_for_shutdown_signal() -> anyhow::Result<()> {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    let terminate = async {
        let mut stream = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        stream.recv().await.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "SIGTERM stream ended before a signal arrived",
            )
        })
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<std::io::Result<()>>();

    match select_shutdown_signal(ctrl_c, terminate).await? {
        ShutdownSignal::Interrupt => eprintln!("Received Ctrl+C, shutting down..."),
        ShutdownSignal::Terminate => eprintln!("Received SIGTERM, shutting down..."),
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_selector_admits_sigterm_to_graceful_cleanup() {
        let selected = select_shutdown_signal(
            std::future::pending::<std::io::Result<()>>(),
            std::future::ready(Ok(())),
        )
        .await
        .expect("SIGTERM selection succeeds");
        assert_eq!(selected, ShutdownSignal::Terminate);
    }

    #[tokio::test]
    async fn shutdown_selector_preserves_ctrl_c_cleanup() {
        let selected = select_shutdown_signal(
            std::future::ready(Ok(())),
            std::future::pending::<std::io::Result<()>>(),
        )
        .await
        .expect("Ctrl+C selection succeeds");
        assert_eq!(selected, ShutdownSignal::Interrupt);
    }
}
