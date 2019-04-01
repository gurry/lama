use powershell_rs::{PsCommand, Stdio, PsProcess, Stdout};
use failure::Fail;
use serde_derive::Deserialize;
use uuid::Uuid;
use std::fmt;
use std::path::Path;
use std::io::{BufReader, BufRead};
use std::collections::HashMap;

pub struct Hyperv;

pub type Result<T> = std::result::Result<T, HypervError>;

impl Hyperv {
    pub fn get_vms() -> Result<Vec<Vm>> {
        let process = Self::spawn("get-vm|select-object -property Id,Name |convertto-json")?;
        let stdout = process.stdout().ok_or_else(|| HypervError::new("Could not access stdout of powershell process"))?;

        let vms: Vec<Vm> = serde_json::from_reader(stdout)
            .map_err(|e| HypervError::new(format!("Failed to parse powershell output: {}", e)))?;

        Ok(vms)
    }

    pub fn import_vm_inplace_new_id<P: AsRef<Path>>(path: P) -> Result<ImportedVm> {
        let path = Self::validate_dir_path(path.as_ref())?;
        let command = &format!(
            r#"$ErrorActionPreference = "Stop";
            $path = "{}";
            $virtual_machines_path = $path;
            $virtual_disks_path = Join-Path $path "Virtual Hard Disks";
            $config_file_path = Get-ChildItem -Path $virtual_machines_path -Filter *.vmcx -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1 | ForEach-Object {{$_.FullName}};
            $report = Compare-Vm -Path $config_file_path -VirtualMachinePath $virtual_machines_path -VhdDestinationPath $virtual_disks_path -GenerateNewId -Copy;

            if ($null -eq $report) {{
                Write-Host "Failed to generate compat report";
                exit 1;
            }}

            $MissingSwitchMsgId = 33012;
            $missing_switches = @{{}};
            foreach ($incompatibilty in $report.Incompatibilities)
            {{
                if ($incompatibilty.MessageId -eq $MissingSwitchMsgId)
                {{
                    $switch_name = $incompatibilty.Message.TrimStart("Could not find Ethernet switch '").TrimEnd("'.");
                    $missing_switches[$incompatibilty.Source.Id] = $switch_name;
                    $incompatibilty.Source |Disconnect-VMNetworkAdapter;
                }}
            }}

            $report = Compare-Vm -CompatibilityReport $report;
            if ($report.Incompatibilities.Length -gt 0) 
            {{
                Write-Host "Failed to resolve all incompatilities";
                exit 2;
            }}

            $vm = Import-VM -CompatibilityReport $report;

            $output = @{{}};
            $output.VmId = $vm.Id;
            $output.MissingSwitches = $missing_switches;

            $output | ConvertTo-Json"#,
        path);

        let stdout = Self::spawn_and_wait(&command)?;

        let vm: ImportedVm = serde_json::from_reader(stdout)
            .map_err(|e| HypervError::new(format!("Failed to parse powershell output: {}", e)))?;

        Ok(vm)
    }

    pub fn compare_vm<P: AsRef<Path>>(path: P, import_type: &ImportType) -> Result<Vec<VmIncompatibility>> {
        let path = Self::validate_file_path(path.as_ref())?;
        let command = format!(
            "$report = compare-vm -Path \"{}\" {};
            if ($?) {{ $report.Incompatibilities | Format-Table -Property MessageId, Message -HideTableHeaders }}",
        path,
        Self::generate_import_vm_param_stub(import_type));
             
        let output = Self::spawn_and_wait(&command)?;

        Self::map_lines(output, |line: &str| {
            let line = line.trim();
            if line.is_empty() {
                return Ok(None)
            }
            let mut parts = line.splitn(2, ' ');
            let msg_id = parts.next().ok_or_else(|| HypervError { msg: "Failed to parse to VmIncomatibility. No MessageId in string".to_owned() })?;
            let msg = parts.next().ok_or_else(|| HypervError { msg: "Failed to parse to VmIncomatibility. No Message in string".to_owned() })?;
            let msg_id = msg_id.parse::<i64>().map_err(|e| HypervError { msg: format!("Failed to parse to VmIncomatibility. Cannot parse MessageId to i64: {}", e) })?;
            Ok(Some(VmIncompatibility::from(msg_id, msg.to_owned())))
        })
    }

    fn generate_import_vm_param_stub(import_type: &ImportType) -> String {
        match import_type {
            ImportType::RegisterInPlace => "".to_owned(),
            ImportType::Restore { vhd_path, virtual_machine_path } => {
                match (vhd_path, virtual_machine_path) {
                    (None, None)  => "-Copy".to_owned(),
                    (Some(vhdpath), None)  => format!("-Copy -VhdDestinationPath \"{}\"", vhdpath.to_string_lossy()),
                    (None, Some(vmpath))  => format!("-Copy -VirtualMachinePath \"{}\"", vmpath.to_string_lossy()),
                    (Some(vhdpath), Some(vmpath))  => format!("-Copy -VhdDestinationPath \"{}\" -VirtualMachinePath \"{}\"", vhdpath.to_string_lossy(), vmpath.to_string_lossy()),
                }
            },
            ImportType::Copy { vhd_path, virtual_machine_path } => {
                match (vhd_path, virtual_machine_path) {
                    (None, None)  => "-GenerateNewId -Copy".to_owned(),
                    (Some(vhdpath), None)  => format!("-GenerateNewId -Copy -VhdDestinationPath \"{}\"", vhdpath.to_string_lossy()),
                    (None, Some(vmpath))  => format!("-GenerateNewId -Copy -VirtualMachinePath \"{}\"", vmpath.to_string_lossy()),
                    (Some(vhdpath), Some(vmpath))  => format!("-GenerateNewId -Copy -VhdDestinationPath \"{}\" -VirtualMachinePath \"{}\"", vhdpath.to_string_lossy(), vmpath.to_string_lossy()),
                }
            }
        }
    }

    fn validate_file_path(path: &Path) -> Result<&str> {
        if !path.is_file() {
            Err(HypervError::new("Path does not point to a valid file"))
        } else {
            let path = path.to_str().ok_or_else(|| HypervError { msg: "Bad path".to_owned() })?;
            Ok(path)
        }
    }

    fn validate_dir_path(path: &Path) -> Result<&str> {
        if !path.is_dir() {
            Err(HypervError::new("Path does not point to a valid directory"))
        } else {
            let path = path.to_str().ok_or_else(|| HypervError { msg: "Bad path".to_owned() })?;
            Ok(path)
        }
    }

    fn map_lines<T, F: Fn(&str) -> Result<Option<T>>>(stdout: Stdout, f: F) -> Result<Vec<T>> {
        let mut vec = Vec::new();
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if let Some(t) = f(&line)? {
                        vec.push(t)
                    }
                }
                Err(e) => Err(HypervError::new(format!("Failed to process powershell output. Could not split stdout into lines: {}", e)))?,
            }
        }

        Ok(vec)
    }

    fn spawn(command: &str) -> Result<PsProcess> {
        PsCommand::new(command)
            .stdout(Stdio::piped()) // TODO: use a tee like mechanism to pipe this to the logger as well when high log level is set
            .spawn()
            .map_err(|e| HypervError::new(format!("Failed to spawn PowerShell process: {}", e)))
    }

    fn spawn_and_wait(command: &str) -> Result<Stdout> {
        let mut process = Self::spawn(command)?;
        let status = process.wait()
            .map_err(|e| HypervError::new(format!("Failed while waiting for PowerShell process: {}", e)))?;

        if !status.success() {
            let exit_code_str = status.code().map(|c| c.to_string()).unwrap_or_else(|| "<none>".to_owned());
            let output = process.wait_with_output() //.map(|c| c.to_string()).unwrap_or_else(|| "<none>".to_owned());
                .map_err(|e| HypervError::new(format!("Failed while waiting for PowerShell process: {}", e)))?;
            let stdout = to_string_truncated(&output.stdout, 1000);
            let stderr = to_string_truncated(&output.stderr, 1000);
            fn handle_blank(s: String) -> String { if !s.is_empty() { s } else { "<empty>".to_owned() } }
            Err(HypervError { msg: format!("Powershell returned failure exit code: {}.\nStdout: {} \nStderr: {}", exit_code_str, handle_blank(stdout), handle_blank(stderr)) })
        } else {
            let output = process.stdout()
                .ok_or_else(|| HypervError::new("Failed obtain stdout of PowerShell process".to_owned()))?;
            Ok(output)
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ImportedVm {
    #[serde(rename = "VmId")]
    pub id: VmId,

    #[serde(rename = "MissingSwitches")]
    pub missing_switches: HashMap<String, String>,

}
pub enum ImportType<'a, 'b> {
    RegisterInPlace,
    Restore { vhd_path: Option<&'a Path>, virtual_machine_path: Option<&'b Path> },
    Copy { vhd_path: Option<&'a Path>, virtual_machine_path: Option<&'b Path> },
}

#[derive(Debug, Deserialize)]
pub struct Vm {
    #[serde(rename = "Id")]
    pub id: VmId,
    #[serde(rename = "Name")]
    pub name: String,
}

// TODO: should this be a newtype?
pub type VmId = Uuid;

#[derive(Debug)]
pub enum VmIncompatibility {
    CannotCreateExternalConfigStore(String),
    TooManyCores(String),
    CannotChangeCheckpointLocation(String),
    CannotChangeSmartPagingStore(String),
    CannotRestoreSavedState(String),
    MissingSwitch(String),
    Other(String, i64),
}

impl VmIncompatibility {
    fn from(msg_id: i64, msg: String) -> Self {
        match msg_id {
            13000 => VmIncompatibility::CannotCreateExternalConfigStore(msg),
            14420 => VmIncompatibility::TooManyCores(msg),
            16350 => VmIncompatibility::CannotChangeCheckpointLocation(msg),
            16352 => VmIncompatibility::CannotChangeSmartPagingStore(msg),
            25014 => VmIncompatibility::CannotRestoreSavedState(msg),
            33012 => VmIncompatibility::MissingSwitch(msg),
            msg_id => VmIncompatibility::Other(msg, msg_id)
        }
    }

    pub fn message_id(&self) -> i64 {
        match self {
            VmIncompatibility::CannotCreateExternalConfigStore(_) => 13000,
            VmIncompatibility::TooManyCores(_) => 14420,
            VmIncompatibility::CannotChangeCheckpointLocation(_) => 16350,
            VmIncompatibility::CannotChangeSmartPagingStore(_) => 16352,
            VmIncompatibility::CannotRestoreSavedState(_) => 25014,
            VmIncompatibility::MissingSwitch(_) => 33012,
            VmIncompatibility::Other(_, i) => *i,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            VmIncompatibility::CannotCreateExternalConfigStore(s) => &s,
            VmIncompatibility::TooManyCores(s) => &s,
            VmIncompatibility::CannotChangeCheckpointLocation(s) => &s,
            VmIncompatibility::CannotChangeSmartPagingStore(s) => &s,
            VmIncompatibility::CannotRestoreSavedState(s) => &s,
            VmIncompatibility::MissingSwitch(s) => &s,
            VmIncompatibility::Other(s, _) => &s,
        }
    }
}

// TODO: We need to do proper design of error types. Just this one type is not enough
#[derive(Debug, Fail)]
pub struct HypervError  {
    pub msg: String,
}

impl HypervError {
    fn new<T: Into<String>>(msg: T) -> Self {
        Self { msg: msg.into() }
    }
}

impl fmt::Display for HypervError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.msg)
    }
}

fn to_string_truncated(bytes: &[u8], take: usize) -> String {
    let len = std::cmp::min(bytes.len(), take);
    String::from_utf8_lossy(&bytes[..len]).to_string()
}