//! Components of Zallet.
//!
//! These are not [`abscissa_core::Component`]s because Abscissa's dependency injection is
//! [buggy](https://github.com/iqlusioninc/abscissa/issues/989).

pub(crate) mod database;
pub(crate) mod json_rpc;
pub(crate) mod keystore;
pub(crate) mod sync;
