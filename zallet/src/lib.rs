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

#[cfg(all(zallet_build = "wallet", zallet_build = "merchant_terminal"))]
compile_error!("zallet_build must only be set to a single value");

pub mod application;
mod cli;
mod commands;
mod components;
pub mod config;
mod error;
mod i18n;
pub mod network;
mod prelude;
mod task;

#[cfg(feature = "zcashd-import")]
mod rosetta;

// Needed for the `Component` derive to work.
use abscissa_core::{Application, Version, component};

// Loads the build-time information.
shadow_rs::shadow!(build);

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
