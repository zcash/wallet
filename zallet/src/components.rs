//! Components of Zallet.
//!
//! These are not [`abscissa_core::Component`]s because Abscissa's dependency injection is
//! [buggy](https://github.com/iqlusioninc/abscissa/issues/989).

use tokio::task::JoinHandle;

use crate::error::Error;

pub(crate) mod chain_view;
pub(crate) mod database;
pub(crate) mod json_rpc;
pub(crate) mod keystore;
pub(crate) mod sync;
pub(crate) mod tracing;

/// A handle to a background task spawned by a component.
///
/// Background tasks in Zallet are either one-shot (expected to terminate before Zallet),
/// or ongoing (Zallet shuts down if the task finishes). The tasks are monitored by
/// [`StartCmd::start`].
///
/// [`StartCmd::start`]: crate::cli::StartCmd::start
pub(crate) type TaskHandle = JoinHandle<Result<(), Error>>;
