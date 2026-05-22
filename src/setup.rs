#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::fs;
use std::io::Write;
use std::process::Command;

const MSI_BYTES: &[u8] = include_bytes!(env!("PHASE_MSI_PATH"));
const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    if std::env::args().any(|arg| arg == "--smoke-test") {
        if MSI_BYTES.len() < 1024 {
            std::process::exit(1);
        }
        return;
    }

    if run_setup().is_err() {
        std::process::exit(1);
    }
}

fn run_setup() -> Result<(), String> {
    let mut msi_path = std::env::temp_dir();
    msi_path.push(format!("PhaseAutoUpdater-{PACKAGE_VERSION}.msi"));

    let mut file =
        fs::File::create(&msi_path).map_err(|error| format!("Could not prepare MSI: {error}"))?;
    file.write_all(MSI_BYTES)
        .map_err(|error| format!("Could not write MSI: {error}"))?;
    file.flush()
        .map_err(|error| format!("Could not finish MSI: {error}"))?;

    let status = Command::new("msiexec")
        .arg("/i")
        .arg(&msi_path)
        .arg("/passive")
        .status()
        .map_err(|error| format!("Could not start installer: {error}"))?;

    if !status.success() {
        return Err(format!("Installer exited with {status}"));
    }

    Ok(())
}
