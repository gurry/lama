mod hyperv;

use std::path::PathBuf;
use structopt::StructOpt;
use quicli::prelude::*;
use hyperv::{Hyperv, SwitchType};
use std::collections::HashMap;

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
    println!("Imported VM with ID {}", vm.id);
    let mut created_switches = HashMap::new();
    for s in vm.missing_switches {
        let adapter_id = s.0;
        let switch_name = s.1;
        println!("Missing Connection: Adapter {}, Switch {}", adapter_id, switch_name);
        let switch_id = if !created_switches.contains_key(&switch_name) {
            let switch_id = Hyperv::create_switch(&switch_name, &SwitchType::Private)?;
            println!("Created switch {}: {}", switch_name, switch_id);
            created_switches.insert(switch_name, switch_id);
            switch_id
        } else {
            created_switches[&switch_name]
        };

        println!("Connecting adapter {} to switch {}", adapter_id, switch_id);
        Hyperv::connect_adapter(&vm.id, &adapter_id, &switch_id.to_hyphenated().to_string())?;
    }
    println!("Starting VM...", );
    Hyperv::start_vm(&vm.id)?;
    std::thread::sleep_ms(4000);
    println!("Stopping VM...", );
    Hyperv::stop_vm(&vm.id)?;
    std::thread::sleep_ms(4000);
    println!("Deleting VM...", );
    Hyperv::delete_vm(&vm.id)?;
    println!("Deleting Switches...", );
    for switch_id in created_switches.values() {
        Hyperv::delete_switch(&switch_id.to_hyphenated().to_string())?;
    }
    Ok(())
}
