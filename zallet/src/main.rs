use clap::Parser;
use i18n_embed::DesktopLanguageRequester;

mod cli;
mod i18n;

#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id)
    }};

    ($message_id:literal, $($args:expr),* $(,)?) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id, $($args), *)
    }};
}

fn main() {
    let requested_languages = DesktopLanguageRequester::requested_languages();
    i18n::load_languages(&requested_languages);

    let _opts = cli::CliOptions::parse();
}
