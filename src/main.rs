mod hyperv;

use std::path::PathBuf;
use structopt::StructOpt;
use quicli::prelude::*;
use hyperv::{Hyperv, SwitchType};

#[derive(Debug, StructOpt)]
enum Subcommand {
    #[structopt(name = "deploy")]
    Deploy {
        path: PathBuf
    },
}

fn main() -> CliResult {
    let Subcommand::Deploy { path } = Subcommand::from_args();
    let vm = Hyperv::import_vm_inplace_new_id(&path)?;
    println!("New VM with ID {}", vm.id);
    for s in vm.missing_switches {
        println!("Adapter {}: Switch {}", s.0, s.1);
        Hyperv::create_switch(s.1, &SwitchType::Private)?;
    }
    Ok(())
}
