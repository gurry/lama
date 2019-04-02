use powershell_rs::{PsCommand, Stdio, PsProcess, Stdout};
use failure::Fail;
use serde_derive::Deserialize;
use uuid::Uuid;
use std::fmt;
use std::path::Path;
use std::collections::HashMap;
use std::io::Read;

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

    pub fn import_vm_inplace_new_id<P: AsRef<Path>, R: Into<Option<RenameAction>>>(path: P, rename_action: R) -> Result<ImportedVm> {
        let rename_action = rename_action.into();
        let (prefix, new_name) = match rename_action {
            None => ("".to_owned(), "".to_owned()),
            Some(RenameAction::NewName(n)) => ("".to_owned(), n),
            Some(RenameAction::AddPrefix(p)) => (p, "".to_owned()),
        };

        // TODO: add powershell statements in the command below to delete old config files and folders
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

            if ($null -eq $vm) {{
                Write-Host "Failed to import VM";
                exit 3;
            }}

            $prefix = "{}";
            $new_name = "{}";

            if (($null -ne $prefix) -and ($prefix -ne "")) {{
                $new_name = $prefix + "_" + $vm.Name;
            }}

            if (($null -ne $new_name) -and ($new_name -ne "")) {{
                Rename-VM -VM $vm -NewName $new_name;
            }}

            $output = @{{}};
            $output.VmId = $vm.Id;
            $output.VmName = $vm.Name;
            $output.MissingSwitches = $missing_switches;

            $output | ConvertTo-Json"#,
        path,
        prefix,
        new_name);

        let stdout = Self::spawn_and_wait(&command)?;

        let vm: ImportedVm = serde_json::from_reader(stdout)
            .map_err(|e| HypervError::new(format!("Failed to parse powershell output: {}", e)))?;

        Ok(vm)
    }

    pub fn start_vm(vm_id: &VmId) -> Result<()> {
        let command = &format!(
            r#"$ErrorActionPreference = "Stop";
            $vm = Get-Vm -Id {0};
            if ($null -eq $vm) {{
                Write-Host "Failed to get vm with Id {0}";
                exit 1;
            }}
            Start-VM -VM $vm"#,
        vm_id);

        Self::spawn_and_wait(&command)?;
        Ok(())
    }

    pub fn stop_vm(vm_id: &VmId) -> Result<()> {
        let command = &format!(
            r#"$ErrorActionPreference = "Stop";
            $vm = Get-Vm -Id {0};
            if ($null -eq $vm) {{
                Write-Host "Failed to get vm with Id {0}";
                exit 1;
            }}
            Stop-VM -VM $vm -Force"#,
        vm_id);

        Self::spawn_and_wait(&command)?;
        Ok(())
    }

    pub fn delete_vm(vm_id: &VmId) -> Result<()> {
        let command = &format!(
            r#"$ErrorActionPreference = "Stop";
            $vm = Get-Vm -Id {0};
            if ($null -eq $vm) {{
                Write-Host "Failed to get vm with Id {0}";
                exit 1;
            }}
            Remove-VM -VM $vm -Force"#,
        vm_id);

        Self::spawn_and_wait(&command)?;
        Ok(())
    }

    pub fn create_switch<S: AsRef<str>>(name: S, switch_type: &SwitchType<S>) -> Result<Uuid> {
        let name = name.as_ref();
        if name.is_empty() {
            return Err(HypervError::new("Empty string is not a legal switch name"));
        }

        let command = &format!(
            r#"$ErrorActionPreference = "Stop";
            $switch = New-VmSwitch -Name "{}" {};
            $switch.Id.ToString()"#,
        name,
        match switch_type {
            SwitchType::Private => "-SwitchType Private".to_owned(),
            SwitchType::Internal => "-SwitchType Internal".to_owned(),
            SwitchType::External(adapter_name) => format!("-SwitchType -NetAdapterName \"{}\"", adapter_name.as_ref()),
        });

        let mut stdout = Self::spawn_and_wait(&command)?;

        let mut uuid = String::new();
        stdout.read_to_string(&mut uuid)
            .map_err(|e| HypervError::new(format!("Failed to parse powershell output: {}", e)))?;
        let uuid = uuid.trim();

        let switch_id = Uuid::parse_str(uuid)
            .map_err(|e| HypervError::new(format!("Failed to parse powershell output: {}", e)))?;

        Ok(switch_id)
    }

    pub fn delete_switch(switch_id: &str) -> Result<()> {
        let command = &format!(
            r#"$ErrorActionPreference = "Stop";
            $switch = Get-VmSwitch -Id {0};
            if ($null -eq $switch) {{
                Write-Host "Failed to get switch with Id {0}";
                exit 1;
            }}
            Remove-VMSwitch -VMSwitch $switch -Force"#,
        switch_id);

        Self::spawn_and_wait(&command)?;
        Ok(())
    }

    pub fn connect_adapter(vm_id: &VmId, adapter_id: &str, switch_id: &str) -> Result<()> {
        let command = &format!(
            r#"$ErrorActionPreference = "Stop";
            $vm = Get-Vm -Id {0};
            if ($null -eq $vm) {{
                Write-Host "Failed to get vm with Id {0}";
                exit 1;
            }}
            $adapter = $vm.NetworkAdapters | Where-Object {{ $_.Id -eq "{1}"}} | Select-Object -First 1;
            if ($null -eq $adapter) {{
                Write-Host "Failed to get vm adapter with Id {1}";
                exit 2;
            }}
            $switch = Get-VmSwitch -Id {2};
            if ($null -eq $switch) {{
                Write-Host "Failed to get switch with Id {2}";
                exit 3;
            }}

            Connect-VMNetworkAdapter -VMNetworkAdapter $adapter -VMSwitch $switch"#,
        vm_id,
        adapter_id,
        switch_id);

        Self::spawn_and_wait(&command)?;
        Ok(())
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
    #[serde(rename = "VmName")]
    pub name: String,
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

pub enum SwitchType<S: AsRef<str>> {
    Private,
    Internal,
    External(S),
}

pub enum RenameAction {
    NewName(String), // TODO; can we find a way to use &str here instead of String
    AddPrefix(String),
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