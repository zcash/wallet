use clap::{builder::Styles, Args, Parser, Subcommand};

use crate::fl;

#[derive(Debug, Parser)]
#[command(author, version)]
#[command(help_template = format!("\
{{before-help}}{{about-with-newline}}
{}{}:{} {{usage}}

{{all-args}}{{after-help}}\
    ",
    Styles::default().get_usage().render(),
    fl!("usage-header"),
    Styles::default().get_usage().render_reset()))]
#[command(next_help_heading = fl!("flags-header"))]
pub(crate) struct CliOptions {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    Run(Run),
}

#[derive(Debug, Args)]
pub(crate) struct Run {}
