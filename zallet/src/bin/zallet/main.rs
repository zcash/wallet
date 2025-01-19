//! Main entry point for Zallet

#![deny(warnings, missing_docs, trivial_casts, unused_qualifications)]
#![forbid(unsafe_code)]

use i18n_embed::DesktopLanguageRequester;

/// Boot Zallet
fn main() {
    zallet::application::boot(DesktopLanguageRequester::requested_languages());
}
