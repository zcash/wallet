//! Zallet Abscissa Application

use std::sync::atomic::{AtomicUsize, Ordering};

use abscissa_core::{
    Application, FrameworkError, StandardPaths,
    application::{self, AppCell},
    config::{self, CfgCell},
    trace,
};
use abscissa_tokio::TokioComponent;
use i18n_embed::unic_langid::LanguageIdentifier;

use crate::{cli::EntryPoint, config::ZalletConfig, i18n};

/// Application state
pub static APP: AppCell<ZalletApp> = AppCell::new();

/// Zallet Application
#[derive(Debug)]
pub struct ZalletApp {
    /// Application configuration.
    config: CfgCell<ZalletConfig>,

    /// Application state.
    state: application::State<Self>,
}

/// Initializes a new application instance.
///
/// By default no configuration is loaded, and the framework state is initialized to a
/// default, empty state (no components, threads, etc).
impl Default for ZalletApp {
    fn default() -> Self {
        Self {
            config: CfgCell::default(),
            state: application::State::default(),
        }
    }
}

impl Application for ZalletApp {
    type Cmd = EntryPoint;
    type Cfg = ZalletConfig;
    type Paths = StandardPaths;

    fn config(&self) -> config::Reader<ZalletConfig> {
        self.config.read()
    }

    fn state(&self) -> &application::State<Self> {
        &self.state
    }

    fn register_components(&mut self, command: &Self::Cmd) -> Result<(), FrameworkError> {
        let mut components = self.framework_components(command)?;
        components.push(Box::new(TokioComponent::from(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name_fn(|| {
                    static ATOMIC_ID: AtomicUsize = AtomicUsize::new(0);
                    let id = ATOMIC_ID.fetch_add(1, Ordering::SeqCst);
                    format!("tokio-worker-{}", id)
                })
                .build()
                .expect("failed to build Tokio runtime"),
        )));
        self.state.components_mut().register(components)
    }

    fn after_config(&mut self, config: Self::Cfg) -> Result<(), FrameworkError> {
        // Configure components
        let mut components = self.state.components_mut();
        components.after_config(&config)?;
        self.config.set_once(config);
        Ok(())
    }

    fn tracing_config(&self, command: &EntryPoint) -> trace::Config {
        if command.verbose {
            trace::Config::verbose()
        } else {
            trace::Config::default()
        }
    }
}

/// Boots the Zallet application, parsing subcommand and options from command-line
/// arguments, and terminating when complete.
pub fn boot(requested_languages: Vec<LanguageIdentifier>) {
    // We load languages here so that the app's CLI usage text can be localized.
    i18n::load_languages(&requested_languages);

    // Now do the normal Abscissa boot.
    abscissa_core::boot(&APP);
}
