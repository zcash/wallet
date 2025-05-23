/// Spawns a new asynchronous task, returning a `JoinHandle` for it.
///
/// This is a wrapper for [`tokio::task::spawn`] that takes a task name.
#[macro_export]
macro_rules! spawn {
    ( $name:expr, $f:expr ) => {
        tokio::task::spawn($f)
    };
}

/// Runs the provided closure on a thread where blocking is acceptable.
///
/// This is a wrapper for [`tokio::task::spawn_blocking`] that takes a task name.
#[macro_export]
macro_rules! spawn_blocking {
    ( $name:expr, $f:expr ) => {
        tokio::task::spawn_blocking($f)
    };
}
