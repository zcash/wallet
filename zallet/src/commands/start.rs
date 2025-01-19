//! `start` subcommand - example of how to write a subcommand

use abscissa_core::{config, FrameworkError, Runnable, Shutdown};

use crate::{
    cli::StartCmd,
    config::ZalletConfig,
    error::{Error, ErrorKind},
    prelude::*,
};

impl StartCmd {
    async fn start(&self) -> Result<(), Error> {
        println!("run");
        Err(ErrorKind::Discombobulated.into())
    }
}

impl Runnable for StartCmd {
    fn run(&self) {
        match abscissa_tokio::run(&APP, self.start()) {
            Ok(Ok(())) => (),
            Ok(Err(e)) => {
                eprintln!("{}", e);
                APP.shutdown(Shutdown::Forced);
            }
            Err(e) => {
                eprintln!("{}", e);
                APP.shutdown(Shutdown::Forced);
            }
        }
    }
}

impl config::Override<ZalletConfig> for StartCmd {
    fn override_config(&self, config: ZalletConfig) -> Result<ZalletConfig, FrameworkError> {
        Ok(config)
    }
}
