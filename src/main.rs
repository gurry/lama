mod hyperv;

use std::path::PathBuf;
use structopt::StructOpt;
use quicli::prelude::*;
use hyperv::{Hyperv, SwitchType, ImportedVm};
use std::collections::HashMap;
use std::path::Path;
use exitfailure::ExitFailure;
use std::fs;
use uuid::Uuid;

#[derive(Debug, StructOpt)]
enum Subcommand {
    #[structopt(name = "deploy")]
    Deploy {
        path: PathBuf
    },
}

fn main() -> CliResult {
    let Subcommand::Deploy { path } = Subcommand::from_args();
    import_lab(path)?;
    Ok(())
}

fn import_lab<P: AsRef<Path>>(path: P) -> CliResult {
    let mut created_switches = HashMap::new();
    let vm_paths = get_vm_paths(&path)?;
    print!("Found {} VMs in lab", vm_paths.len());
    if vm_paths.is_empty() {
        println!(". Nothing to deploy");
        return Ok(());
    } else {
        println!("");
    }

    for vm_path in &vm_paths {
        import_vm(vm_path, &mut created_switches)?;
    }

    println!("Lab deployed successfully");
    Ok(())
}

fn get_vm_paths<P: AsRef<Path>>(path: P) -> Result<Vec<PathBuf>, ExitFailure> {
    let mut vm_paths = Vec::new();
    for entry in fs::read_dir(path)? {
        let path = entry?.path();
        if path.is_dir() && contains_vmcx_file(&path)? {
            vm_paths.push(path);
        }
    }
    Ok(vm_paths)
}

fn contains_vmcx_file(path: &Path) -> Result<bool, ExitFailure> {
    // We don't check path is a directory. We just assume it is.
    let vm_config_dir = path.join("Virtual Machines");
    if vm_config_dir.exists() {
        for entry in fs::read_dir(vm_config_dir)? {
            let path = entry?.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if let Some(ext) = ext.to_str() {
                        if ext.to_lowercase() == "vmcx" {
                            return Ok(true)
                        }
                    }
                }
            }
        }
    }

    Ok(false)
}

fn import_vm<P: AsRef<Path>>(path: P, created_switches: &mut HashMap<String, Uuid>) -> Result<ImportedVm, ExitFailure> {
    let path = path.as_ref();
    let vm_folder_name = path.file_name().expect("Bad VM folder name").to_str().expect("Couldn't convert VM folder name to str");  // TODO: replace the expect with an error
    print!("Importing VM {}... ", vm_folder_name);
    let vm = Hyperv::import_vm_inplace_new_id(&path, None)?;
    println!("Done (ID: {})", vm.id);
    for s in &vm.adapter_status {
        let adapter_id = s.0;
        let switch_name = &s.1.name;
        let switch_id = if !created_switches.contains_key(switch_name) {
            print!("==> {}: Creating switch '{}'... ", vm.name, switch_name);
            let switch_id = Hyperv::create_switch(switch_name, &SwitchType::Private)?;
            println!("Done (ID: {})", switch_id);
            created_switches.insert(switch_name.to_owned(), switch_id);
            switch_id
        } else {
            created_switches[switch_name]
        };

        print!("==> {}: Connecting to switch '{}'... ", vm.name, switch_name);
        Hyperv::connect_adapter(&vm.id, &adapter_id, &switch_id.to_hyphenated().to_string())?;
        println!("Done");
    }

    print!("==> {}: Starting VM... ", vm.name);
    Hyperv::start_vm(&vm.id)?;
    println!("Done");

    Ok(vm)
}
