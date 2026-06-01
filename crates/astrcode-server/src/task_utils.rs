use std::future::Future;

/// Spawn a background task and log panics/cancellation.
///
/// This avoids "fire-and-forget" tasks silently disappearing: if the task panics,
/// the join error is observed and logged.
pub fn spawn_traced(
    name: &'static str,
    fut: impl Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let inner = tokio::spawn(fut);
        match inner.await {
            Ok(()) => {},
            Err(e) => {
                if e.is_panic() {
                    tracing::error!(task = name, "background task panicked");
                } else {
                    tracing::warn!(task = name, error = %e, "background task cancelled");
                }
            },
        }
    })
}
