//! Zallet
//!
//! Application based on the [Abscissa] framework.
//!
//! [Abscissa]: https://github.com/iqlusioninc/abscissa

#![forbid(unsafe_code)]
#![deny(rustdoc::broken_intra_doc_links)]
#![warn(
    missing_docs,
    rust_2018_idioms,
    unused_lifetimes,
    unused_qualifications
)]

pub mod application;
mod cli;
mod commands;
mod components;
pub mod config;
mod error;
mod i18n;
pub mod network;
mod prelude;

// Needed for the `Component` derive to work.
use abscissa_core::{Application, Version, component};

/// A macro to obtain localized Zallet messages and optionally their attributes, and check
/// the `message_id`, `attribute_id` and arguments at compile time.
///
/// See [`i18n_embed_fl::fl`] for full documentation.
#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id)
    }};

    ($message_id:literal, $($args:expr),* $(,)?) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id, $($args), *)
    }};
}
