#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
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
        .arg("START_ON_LOGIN=1")
        .status()
        .map_err(|error| format!("Could not start installer: {error}"))?;

    if !status.success() {
        return Err(format!("Installer exited with {status}"));
    }

    launch_installed_app()
}

fn launch_installed_app() -> Result<(), String> {
    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        return Ok(());
    };

    let app_path = PathBuf::from(local_app_data)
        .join("Programs")
        .join("Phase Auto Updater")
        .join("PhaseAnimatorInstaller.exe");

    if !app_path.exists() {
        return Ok(());
    }

    Command::new(app_path)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not launch Phase Auto Updater: {error}"))
}
