//! `example-config` subcommand

use abscissa_core::{Runnable, Shutdown};
use tokio::{fs::File, io::AsyncWriteExt};

use crate::{
    cli::ExampleConfigCmd,
    config::ZalletConfig,
    error::{Error, ErrorKind},
    fl,
    prelude::*,
};

impl ExampleConfigCmd {
    async fn start(&self) -> Result<(), Error> {
        if !self.this_is_alpha_code_and_you_will_need_to_recreate_the_example_later {
            return Err(ErrorKind::Generic.context(fl!("example-alpha-code")).into());
        }

        // Serialize the example config.
        let output = ZalletConfig::generate_example();

        // Write the Zallet config file.
        let output_path = match self.output.as_deref() {
            None => todo!("No default Zallet config path yet, use -o/--output"),
            Some("-") => None,
            Some(path) => Some(path),
        };
        if let Some(path) = output_path {
            let mut f = if self.force {
                File::create(path).await
            } else {
                File::create_new(path).await
            }
            .map_err(|e| ErrorKind::Generic.context(e))?;
            f.write_all(output.as_bytes())
                .await
                .map_err(|e| ErrorKind::Generic.context(e))?;
            println!("{}", fl!("migrate-config-written", conf = path));
        } else {
            println!("{output}")
        }

        Ok(())
    }
}

impl Runnable for ExampleConfigCmd {
    fn run(&self) {
        match abscissa_tokio::run(&APP, self.start()) {
            Ok(Ok(())) => (),
            Ok(Err(e)) => {
                eprintln!("{e}");
                APP.shutdown_with_exitcode(Shutdown::Forced, 1);
            }
            Err(e) => {
                eprintln!("{e}");
                APP.shutdown_with_exitcode(Shutdown::Forced, 1);
            }
        }
    }
}
