use abscissa_core::{Command, Runnable};
use clap::{builder::Styles, Parser};

use crate::fl;

#[derive(Debug, Parser, Command)]
#[command(author, about, version)]
#[command(help_template = format!("\
{{before-help}}{{about-with-newline}}
{}{}:{} {{usage}}

{{all-args}}{{after-help}}\
    ",
    Styles::default().get_usage().render(),
    fl!("usage-header"),
    Styles::default().get_usage().render_reset()))]
#[command(next_help_heading = fl!("flags-header"))]
pub struct EntryPoint {
    #[command(subcommand)]
    pub(crate) cmd: ZalletCmd,

    /// Enable verbose logging
    #[arg(short, long)]
    pub(crate) verbose: bool,

    /// Use the specified config file
    #[arg(short, long)]
    pub(crate) config: Option<String>,
}

#[derive(Debug, Parser, Command, Runnable)]
pub(crate) enum ZalletCmd {
    /// The `start` subcommand
    Start(StartCmd),
}

/// `start` subcommand
#[derive(Debug, Parser, Command)]
pub(crate) struct StartCmd {}
