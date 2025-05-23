use tracing_log::LogTracer;
use tracing_subscriber::FmtSubscriber;

use abscissa_core::{Component, FrameworkError, FrameworkErrorKind, terminal::ColorChoice};

/// Abscissa component for initializing the `tracing` subsystem
#[derive(Component, Debug)]
#[component(core)]
pub(crate) struct Tracing {}

impl Tracing {
    pub(crate) fn new(color_choice: ColorChoice) -> Result<Self, FrameworkError> {
        let filter = std::env::var("RUST_LOG").unwrap_or("info".to_owned());

        // Configure log/tracing interoperability by setting a `LogTracer` as
        // the global logger for the log crate, which converts all log events
        // into tracing events.
        LogTracer::init().map_err(|e| FrameworkErrorKind::ComponentError.context(e))?;

        // Construct a tracing subscriber with the supplied filter and enable reloading.
        let builder = FmtSubscriber::builder()
            .with_ansi(match color_choice {
                ColorChoice::Always => true,
                ColorChoice::AlwaysAnsi => true,
                ColorChoice::Auto => true,
                ColorChoice::Never => false,
            })
            .with_env_filter(filter);
        let subscriber = builder.finish();

        // Now set it as the global tracing subscriber and save the handle.
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|e| FrameworkErrorKind::ComponentError.context(e))?;

        Ok(Self {})
    }
}
