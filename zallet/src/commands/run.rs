use crate::{cli::Run, error::Error};

impl Run {
    pub(crate) fn run(self) -> Result<(), Error> {
        println!("run");
        Err(Error::Discombobulated)
    }
}
