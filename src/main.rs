use std::path::PathBuf;
use structopt::StructOpt;
use quicli::prelude::*;
use hyperv_rs::Hyperv;

#[derive(Debug, StructOpt)]
enum Subcommand {
    #[structopt(name = "deploy")]
    Deploy {
        path: PathBuf
    },
}

fn main() -> CliResult {
    let Subcommand::Deploy { path } = Subcommand::from_args();
    Hyperv::import_vm(&path)?;
    Ok(())
}
