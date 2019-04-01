use powershell_rs::{PsCommand, Stdio, PsProcess, Stdout};
use failure::Fail;
use serde_derive::Deserialize;
use uuid::Uuid;
use std::fmt;
use std::path::Path;
use std::collections::HashMap;

pub struct Hyperv;

pub type Result<T> = std::result::Result<T, HypervError>;

impl Hyperv {
    pub fn get_vms() -> Result<Vec<Vm>> {
        let process = Self::spawn(r#"$ErrorActionPreference = "Stop";get-vm|select-object -property Id,Name |convertto-json"#)?;
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

    fn validate_dir_path(path: &Path) -> Result<&str> {
        if !path.is_dir() {
            Err(HypervError::new("Path does not point to a valid directory"))
        } else {
            let path = path.to_str().ok_or_else(|| HypervError { msg: "Bad path".to_owned() })?;
            Ok(path)
        }
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

#[derive(Debug, Deserialize)]
pub struct Vm {
    #[serde(rename = "Id")]
    pub id: VmId,
    #[serde(rename = "Name")]
    pub name: String,
}

// TODO: should this be a newtype?
pub type VmId = Uuid;

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