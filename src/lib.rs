extern crate env_logger;
extern crate failure;
#[macro_use]
extern crate log;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate serde_json;

mod msg;

use failure::Error;

pub fn run() -> Result<(), Error> {
    env_logger::init()?;
    info!("Starting up");
    Ok(())
}
