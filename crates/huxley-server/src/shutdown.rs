//! Shutdown signal handling.

use tokio_util::sync::CancellationToken;

/// Cancels `token` on SIGINT (Ctrl-C) or SIGTERM (container stop).
pub fn spawn_signal_listener(token: CancellationToken) {
    tokio::spawn(async move {
        let ctrl_c = async {
            if let Err(e) = tokio::signal::ctrl_c().await {
                tracing::error!(error = %e, "failed to listen for Ctrl-C");
                std::future::pending::<()>().await;
            }
        };

        #[cfg(unix)]
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to listen for SIGTERM");
                    std::future::pending::<()>().await;
                }
            }
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            () = ctrl_c => tracing::info!("received Ctrl-C, shutting down"),
            () = terminate => tracing::info!("received SIGTERM, shutting down"),
        }
        token.cancel();
    });
}
