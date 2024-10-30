use clap::Parser;
use i18n_embed::DesktopLanguageRequester;

mod cli;
mod commands;
mod error;
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
fn main() -> Result<(), error::Error> {
    let requested_languages = DesktopLanguageRequester::requested_languages();
    i18n::load_languages(&requested_languages);

    let opts = cli::CliOptions::parse();

    match opts.command {
        cli::Command::Run(cmd) => cmd.run(),
    }
}
