//! Stdio lifecycle for the read tool server.
//!
//! The runtime owns the RMCP transport and the process lifecycle only: it serves
//! one already-composed [`ReadToolServer`] over the standard input/output
//! transport and waits for a clean shutdown. It names no provider adapter, so
//! the production composition stays in the binary root and the SDK transport
//! stays isolated from the application and core contracts.
//!
//! # Protocol discipline
//!
//! Every byte the transport writes goes to stdout and is a JSON-RPC frame; this
//! module never writes a diagnostic there. Failures are returned as a sanitized
//! [`AppError`] so the binary root can report them on stderr.

use destiny_app::{AppError, ErrorCategory};
use rmcp::ServiceExt;
use rmcp::service::QuitReason;

use crate::server::ReadToolServer;

/// Serves the read tool server over stdio until the client closes it.
///
/// The call resolves after the RMCP initialize handshake and returns once the
/// service loop ends, either because the peer closed the transport or because
/// the service was cancelled. A failed initialization or an abnormal join is
/// reported as a sanitized configuration-category failure with no transport or
/// provider text.
pub async fn serve_stdio(server: ReadToolServer) -> Result<(), AppError> {
    let running = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|_| runtime_failure())?;
    match running.waiting().await {
        Ok(QuitReason::JoinError(_)) | Err(_) => Err(runtime_failure()),
        Ok(_) => Ok(()),
    }
}

/// The single sanitized runtime failure category.
fn runtime_failure() -> AppError {
    AppError::new(ErrorCategory::ConfigurationError)
}
