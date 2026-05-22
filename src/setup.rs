#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;

const MSI_BYTES: &[u8] = include_bytes!(env!("PHASE_MSI_PATH"));
const PACKAGE_VERSION: &str = env!("CARGO_PKG_VERSION");
const INSTALLER_TITLE: &str = "Phase Animator Setup";

fn main() {
    if std::env::args().any(|arg| arg == "--smoke-test") {
        if MSI_BYTES.len() < 1024 {
            std::process::exit(1);
        }
        return;
    }

    if let Err(error) = run_setup() {
        show_error(&error);
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
    drop(file);

    let log_path = setup_log_path();
    let mut command = Command::new("msiexec");
    command.arg("/i").arg(&msi_path).arg("START_ON_LOGIN=1");
    if installed_app_path().is_some_and(|path| path.exists()) {
        command.arg("REINSTALL=ALL").arg("REINSTALLMODE=amus");
    }
    let status = command
        .arg("/l*v")
        .arg(&log_path)
        .status()
        .map_err(|error| format!("Could not start installer: {error}"))?;

    if !is_success_status(status.code()) {
        return Err(format!(
            "Installer exited with {status}. Log: {}",
            log_path.display()
        ));
    }

    if let Err(error) = refresh_desktop_shortcut() {
        let _ = fs::write(setup_note_path("shortcut"), error);
    }

    if let Err(error) = launch_installed_app() {
        let _ = fs::write(setup_note_path("launch"), error);
    }

    Ok(())
}

fn launch_installed_app() -> Result<(), String> {
    let Some(app_path) = installed_app_path() else {
        return Ok(());
    };

    if !app_path.exists() {
        return Ok(());
    }

    let work_dir = app_path.parent().map(|path| path.to_path_buf());
    let mut command = Command::new(&app_path);
    if let Some(work_dir) = work_dir {
        command.current_dir(work_dir);
    }

    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Could not launch Phase Auto Updater: {error}"))
}

fn installed_app_path() -> Option<PathBuf> {
    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        return None;
    };

    let app_path = PathBuf::from(local_app_data)
        .join("Programs")
        .join("Phase Auto Updater")
        .join("PhaseAnimatorInstaller.exe");
    Some(app_path)
}

fn is_success_status(code: Option<i32>) -> bool {
    matches!(code, Some(0 | 1641 | 3010))
}

fn setup_log_path() -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("PhaseAnimatorSetup-{PACKAGE_VERSION}.log"));
    path
}

fn setup_note_path(kind: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!("PhaseAnimatorSetup-{PACKAGE_VERSION}.{kind}.txt"));
    path
}

fn refresh_desktop_shortcut() -> Result<(), String> {
    let Some(app_path) = installed_app_path() else {
        return Ok(());
    };
    if !app_path.exists() {
        return Ok(());
    }

    let icon_path = app_path
        .parent()
        .map(|path| path.join("PhaseAnimator.ico"))
        .unwrap_or_else(|| app_path.clone());
    let script = format!(
        "$desktop=[Environment]::GetFolderPath('DesktopDirectory');\
         $target={};\
         $icon={};\
         $link=Join-Path $desktop 'Phase Auto Updater.lnk';\
         $shell=New-Object -ComObject WScript.Shell;\
         $shortcut=$shell.CreateShortcut($link);\
         $shortcut.TargetPath=$target;\
         $shortcut.WorkingDirectory=Split-Path $target;\
         $shortcut.IconLocation=$icon;\
         $shortcut.Save();",
        powershell_string(&app_path),
        powershell_string(&icon_path)
    );

    let status = Command::new("powershell")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-WindowStyle")
        .arg("Hidden")
        .arg("-Command")
        .arg(script)
        .status()
        .map_err(|error| format!("Could not create desktop shortcut: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("Desktop shortcut command exited with {status}"))
    }
}

fn powershell_string(path: &std::path::Path) -> String {
    let escaped = path.to_string_lossy().replace('\'', "''");
    format!("'{escaped}'")
}

#[cfg(target_os = "windows")]
fn show_error(message: &str) {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

    let body = wide(message);
    let title = wide(INSTALLER_TITLE);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value).encode_wide().chain(Some(0)).collect()
    }
}

#[cfg(not(target_os = "windows"))]
fn show_error(message: &str) {
    eprintln!("{INSTALLER_TITLE}: {message}");
}
