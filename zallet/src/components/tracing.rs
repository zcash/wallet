use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Mutex;

use tracing::level_filters::LevelFilter;
use tracing_log::LogTracer;
use tracing_subscriber::{EnvFilter, Layer, fmt::writer::BoxMakeWriter, layer::SubscriberExt};

use abscissa_core::{Component, FrameworkError, FrameworkErrorKind, terminal::ColorChoice};

/// Where the `tracing` subsystem should write its output.
pub(crate) enum LogTarget<'a> {
    /// Write to standard error (the default).
    Stderr,
    /// Write to a file at the given path.
    ///
    /// This is used by the interactive terminal UI, which takes over the terminal and
    /// would otherwise have its display corrupted by interleaved log output.
    File(&'a Path),
}

/// Abscissa component for initializing the `tracing` subsystem
#[derive(Component, Debug)]
#[component(core)]
pub(crate) struct Tracing {}

impl Tracing {
    pub(crate) fn new(
        color_choice: ColorChoice,
        target: LogTarget<'_>,
    ) -> Result<Self, FrameworkError> {
        let env_filter = EnvFilter::builder()
            .with_default_directive(LevelFilter::INFO.into())
            .from_env_lossy();

        // Configure log/tracing interoperability by setting a `LogTracer` as
        // the global logger for the log crate, which converts all log events
        // into tracing events.
        LogTracer::init().map_err(|e| FrameworkErrorKind::ComponentError.context(e))?;

        // Select the writer and whether to emit ANSI colour codes. When writing to a
        // file, colour codes are always disabled so the log remains readable.
        let (writer, ansi): (BoxMakeWriter, bool) = match target {
            LogTarget::Stderr => (
                BoxMakeWriter::new(io::stderr),
                match color_choice {
                    ColorChoice::Always => true,
                    ColorChoice::AlwaysAnsi => true,
                    ColorChoice::Auto => true,
                    ColorChoice::Never => false,
                },
            ),
            LogTarget::File(path) => {
                let file = File::create(path)
                    .map_err(|e| FrameworkErrorKind::ComponentError.context(e))?;
                (BoxMakeWriter::new(Mutex::new(file)), false)
            }
        };

        // Construct a tracing subscriber with the supplied filter and enable reloading.
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_writer(writer)
            .with_ansi(ansi)
            .with_filter(env_filter);

        let subscriber = tracing_subscriber::registry().with(fmt_layer);

        // Spawn the console server in the background, and apply the console layer.
        #[cfg(all(feature = "tokio-console", tokio_unstable))]
        let subscriber = subscriber.with(console_subscriber::spawn());

        // Now set it as the global tracing subscriber and save the handle.
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|e| FrameworkErrorKind::ComponentError.context(e))?;

        Ok(Self {})
    }
}
