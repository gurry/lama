mod hyperv;

use crate::hyperv::VmId;
use std::path::PathBuf;
use structopt::StructOpt;
use quicli::prelude::*;
use hyperv::{Hyperv, SwitchType, ImportedVm};
use std::collections::HashMap;
use std::path::{Path, Component, Prefix};
use exitfailure::ExitFailure;
use std::fs;
use std::fmt;
use std::io::{stdin, stdout, Write};
use uuid::Uuid;
use failure::Fail;
use fs_extra::{copy_items_with_progress, copy_items, dir::{CopyOptions, TransitProcessResult}};
use pbr::ProgressBar;

#[derive(Debug, StructOpt)]
enum Subcommand {
    #[structopt(name = "deploy")]
    Deploy { 
        path: PathBuf,
        #[structopt(long = "provision")]
        provisioner_path: Option<PathBuf>
    },
    #[structopt(name = "drop")]
    Delete { path: PathBuf },
}

fn main() -> CliResult {
    match Subcommand::from_args() {
        Subcommand::Deploy { path, provisioner_path } => deploy_lab(path, provisioner_path)?,
        Subcommand::Delete { path } => delete_lab(path)?,
    }
    
    Ok(())
}

fn deploy_lab(mut lab_path: PathBuf, _provisioner_path: Option<PathBuf>) -> CliResult {
    if !lab_path.is_dir() {
        return Err(LamaError::new(format!("Path '{}' does not exist", lab_path.display())))?;
    }

    let lab_folder_name = lab_path.file_name();

    const YES_CHOICE: &str = "Y";
    const NO_CHOICE: &str = "N";
    if lab_folder_name.is_none() {
        let prompt = format!("'{}' does not seem to be a valid lab path. Are you sure you want to deploy from here? [{}] Yes [{}] No: ", lab_path.display(), YES_CHOICE, NO_CHOICE);
        if prompt_user(prompt.as_str())?.as_str() != YES_CHOICE {
            return Ok(());
        }
    }

    if is_remote_path(&lab_path)? {
        println!("Cannot deploy from network location. Do you want me to copy the lab locally first and deploy from there?");
        const DIFFERENT_LOC_CHOICE: &str = "D";
        let dest_path: PathBuf = match prompt_user(&format!("[{}] Copy to current directory [{}] Copy to a different location [{}] Abort: ", YES_CHOICE, DIFFERENT_LOC_CHOICE, NO_CHOICE))?.to_uppercase().as_str() {
            YES_CHOICE => ".".to_owned(),
            DIFFERENT_LOC_CHOICE => {
                prompt_user("Enter path (will be created if missing): ")?
            }
            NO_CHOICE => return Ok(()),
            _ => {
                return Err(LamaError::new("Invalid choice"))?;
            }
        }.into();

        if !dest_path.is_dir() {
            fs::create_dir_all(&dest_path)?;
            println!("Created directory {}", dest_path.display());
        } 

        let full_dest_path = match lab_folder_name {
            Some(folder_name) => PathBuf::from(&dest_path).join(folder_name),
            None => PathBuf::from(&dest_path),
        };
        println!("Copying to {}...", full_dest_path.display());
        copy_lab(&lab_path, &dest_path)?;
        lab_path = match lab_folder_name {
            Some(folder_name) => PathBuf::from(dest_path).join(folder_name),
            None => PathBuf::from(dest_path),
        };
    }

    // TOOD: for non-remote lab paths make sure that lab is not already deployed
    import_lab(lab_path)?;
    Ok(())
}

fn delete_lab<P: AsRef<Path>>(path: P) -> CliResult {
    let lab_path = path.as_ref();
    if !lab_path.is_dir() {
        return Err(LamaError::new(format!("Path '{}' does not exist", lab_path.display())))?;
    }

    for vm_path in get_vm_paths(lab_path)? {
        let vm_id = get_vm_id(&vm_path)?;
        if let Some(vm_id) = vm_id {
            println!("==> Stopping VM {}... ", vm_id);
            Hyperv::stop_vm(&vm_id)?;
            print!("==> Deleting VM {}... ", vm_id);
            back_up_vm_config(&vm_path)?; // Save the VM config files because delete-vm will delete them
            if Hyperv::delete_vm(&vm_id)? {
                println!("deleted");
            } else {
                println!("not found");
            }
            restore_vm_config(&vm_path)?; // Restore the backed up config files now
        }
    }

    // Delete switches if .lama/switches.json file is present
    let lama_dir_path = lab_path.join(".lama");
    let switches_file_path = lama_dir_path.join("switches.json");
    if switches_file_path.is_file() {
        let mut switches_file = fs::File::open(&switches_file_path)?;
        let switches: HashMap<String, Uuid> = serde_json::from_reader(&mut switches_file)?;

        for switch_name in switches.keys() {
            // TODO: check if the switch is connected to any VM an don't delete it if it is
            print!("==> Deleting switch {}... ", switch_name);
            let switch_id = switches[switch_name];
            if Hyperv::delete_switch(&switch_id.to_hyphenated().to_string())? {
                println!("deleted");
            } else {
                println!("not found");
            }
        }

        // TODO: write the new switches file here based on switches being used just before the 'drop'
        // so that next time 'deploy' is called the same envionrment as now is recreated.
    }

    Ok(())
}

fn copy_lab<S: AsRef<Path>, D: AsRef<Path>>(source_path: S, dest_path: D) -> CliResult {
    let count = 100;
    let mut pb = ProgressBar::new(count);
    pb.show_counter = false;
    pb.show_speed = false;
    pb.format("[=>-]");

    let options = CopyOptions::new(); //Use default values for CopyOptions
    let mut from_paths = Vec::new();
    from_paths.push(source_path);
    let _bytes = copy_items_with_progress(&from_paths, dest_path, &options, |process_info| {
        pb.total = process_info.total_bytes;
        pb.set(process_info.copied_bytes);
        if process_info.copied_bytes == process_info.total_bytes {
            pb.finish_print("done");
        }

        TransitProcessResult::ContinueOrAbort
    })?;
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

    let lama_config_folder_path = path.as_ref().join(".lama");

    if !lama_config_folder_path.is_dir() {
        fs::create_dir_all(&lama_config_folder_path)?;
    }

    let mut switches_file = fs::File::create(lama_config_folder_path.join("switches.json"))?;
    serde_json::to_writer(&mut switches_file, &created_switches)?;

    println!("Lab deployed successfully");
    Ok(())
}

fn run_provisioner<P: AsRef<Path>>(vm_ids: Vec<VmId>, provisioner_path: P) -> CliResult {
    // TODO :this is how to implement this method:
    // 1. For each VM get Get the ipv6 link-local address.
    // Why ipv6 link-local address? Because a. it is guaranteed
    // to be there (IPv6 standard requires them to be always present)
    // and b. they are _almost_ guaranteed to be unique amongst all VMs
    // on the host machine including those in other labs. The same
    // can't be said for regular ipv4 addresess. And uniqueness is
    // important because otherwise you might end up provisioning a
    // completely wrong VM.
    // 2. Add this address to the TrustedHosts setting of local WinRM
    // This is because we'll be communicating with the VM over WinRM
    // using this IP and as per WinRM requirements you can't communicate
    // with an IP unless it's added to the TrustedHosts setting.
    // Before we do this we'd also want to check that the WinRm service
    // is running on the host machine because otherwise adding to the
    // TrustedHosts setting is going to fail. If the service isn't there
    // then fail with an error.
    // 3. Now run the provision script given at the 'provisioner_path'.
    // Pass this script the list of all VM objects in this lab and 
    // their above-mentioned ipv6 link-local addresses. The script will
    // most likely make a PsSession with the VM and do whatever it wants
    // to do on it.
    // See this PR to know how Vagrant people do something similar:
    // https://github.com/hashicorp/vagrant/pull/4400/files
    // except that they don't use ipv6 link-local addresses like us.

    Ok(())
}

fn get_vm_paths<P: AsRef<Path>>(path: P) -> Result<Vec<PathBuf>, ExitFailure> {
    let mut vm_paths = Vec::new();
    for entry in fs::read_dir(path)? {
        let vm_path = entry?.path();
        if vm_path.is_dir() {
            if has_vmcx_file(&vm_path)? {
                 vm_paths.push(vm_path);
            }
        }
    }

    Ok(vm_paths)
}

fn get_vm_id<P: AsRef<Path>>(path: P) -> Result<Option<VmId>, ExitFailure> {
    let vmcx_path = get_single_vmcx_file_path(path.as_ref())?;
    let vm_id = match vmcx_path.map(|p| p.file_stem().map(|p| p.to_str().map(|p| VmId::parse_str(p).ok()))) {
        Some(Some(Some(opt))) => opt,
        _ => None
    };

    Ok(vm_id)
}

fn has_vmcx_file(vm_dir: &Path) -> Result<bool, ExitFailure> {
    Ok(get_single_vmcx_file_path(&vm_dir)?.is_some())
}

// Returns error or there are more than one vcmx files
fn get_single_vmcx_file_path(vm_dir: &Path) -> Result<Option<PathBuf>, ExitFailure> {
    let mut paths = get_vmcx_file_paths(vm_dir)?;
    if paths.len() > 1 {
        Err(LamaError::new("More than one .vmcx files found".to_owned()))?
    } else if paths.len() == 1 {
        let path = paths.remove(0); // Must remove first because otherwise we upset the borrowck
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

fn get_vmcx_file_paths(vm_dir: &Path) -> Result<Vec<PathBuf>, ExitFailure> {
    // We don't check path is a directory. We just assume it is.
    let vm_config_dir = vm_dir.join("Virtual Machines");
    let mut vmcx_paths = Vec::new();
    if vm_config_dir.exists() {
        for entry in fs::read_dir(vm_config_dir)? {
            let path = entry?.path();
            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if let Some(ext) = ext.to_str() {
                        if ext.to_lowercase() == "vmcx" {
                            vmcx_paths.push(path);
                        }
                    }
                }
            }
        }
    }

    Ok(vmcx_paths)
}

fn back_up_vm_config(vm_dir: &Path) -> Result<(), ExitFailure> {
    let lama_dir = vm_dir.join(".lama");
    let vm_config_dir = vm_dir.join("Virtual Machines");
    copy_dir_contents(&vm_config_dir, &lama_dir)?;
    Ok(())
}

fn restore_vm_config(vm_dir: &Path) -> Result<(), ExitFailure> {
    let lama_dir = vm_dir.join(".lama");
    let vm_config_dir = vm_dir.join("Virtual Machines");
    copy_dir_contents(&lama_dir, &vm_config_dir)?;
    fs::remove_dir_all(&lama_dir)?;
    Ok(())
}

fn copy_dir_contents(src_dir: &Path, dest_dir: &Path) -> Result<(), ExitFailure> {
    if src_dir.is_dir() {
        if dest_dir.is_dir() {
            remove_dir_contents(&dest_dir)?; // Just to remove any pre-existing junk
        } else {
            fs::create_dir(&dest_dir)?;
        }

        let mut src_paths = Vec::new();
        for entry in fs::read_dir(&src_dir)? {
            src_paths.push(entry?.path());
        }

        let options = CopyOptions::new(); //Use default values for CopyOptions
        copy_items(&src_paths, &dest_dir, &options)?;
    }

    Ok(())
}

fn remove_dir_contents(dir: &Path) -> Result<(), ExitFailure> {
    if dir.exists() {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                fs::remove_dir_all(path)?;
            } else if path.is_file() {
                fs::remove_file(path)?;
            }
        }
    }

    Ok(())
}

fn import_vm<P: AsRef<Path>>(path: P, created_switches: &mut HashMap<String, Uuid>) -> Result<ImportedVm, ExitFailure> {
    let path = path.as_ref();
    let vm_folder_name = path.file_name()
        .ok_or_else(|| LamaError::new("Bad VM folder name"))?
        .to_str()
        .ok_or_else(|| LamaError::new("Couldn't convert VM folder name to str"))?;
 
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

fn is_remote_path(path: &Path) -> Result<bool, ExitFailure> {
    let res = match path.components().next() {
        Some(Component::Prefix(prefix_component)) => match prefix_component.kind() {
            Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _) => true, // TODO: also cater for network paths that point to localhost or 127.0.0.x
            _ => false,
        },
        _ => false,
    };

    Ok(res)
}

pub fn prompt_user(prompt: &str) -> Result<String, ExitFailure> {
    print!("{}", prompt);
    let _= stdout().flush()?;
    let mut input = String::new();
    stdin().read_line(&mut input)?;
    Ok(input.trim().to_owned())
}


#[derive(Debug, Fail)]
pub struct LamaError(String);

impl LamaError {
    fn new<T: Into<String>>(msg: T) -> Self {
        Self(msg.into())
    }
}

impl fmt::Display for LamaError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}