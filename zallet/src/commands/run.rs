use crate::{cli::Run, error::Error};

impl Run {
    pub(crate) async fn run(self) -> Result<(), Error> {
        println!("run");
        Err(Error::Discombobulated)
    }
}
