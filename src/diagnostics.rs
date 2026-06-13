use crate::verification;
use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticStatus {
    Good,
    Warning,
    Problem,
}

impl DiagnosticStatus {
    pub fn label(self) -> &'static str {
        match self {
            DiagnosticStatus::Good => "Good",
            DiagnosticStatus::Warning => "Check",
            DiagnosticStatus::Problem => "Problem",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DiagnosticCheck {
    pub status: DiagnosticStatus,
    pub title: String,
    pub detail: String,
    pub next_step: String,
    pub raw_error: Option<String>,
    pub elapsed_ms: Option<u128>,
}

#[derive(Clone, Debug)]
pub struct DiagnosticReport {
    pub summary: String,
    pub generated_at: SystemTime,
    pub checks: Vec<DiagnosticCheck>,
}

impl DiagnosticReport {
    pub fn overall_status(&self) -> DiagnosticStatus {
        if self
            .checks
            .iter()
            .any(|check| check.status == DiagnosticStatus::Problem)
        {
            DiagnosticStatus::Problem
        } else if self
            .checks
            .iter()
            .any(|check| check.status == DiagnosticStatus::Warning)
        {
            DiagnosticStatus::Warning
        } else {
            DiagnosticStatus::Good
        }
    }

    pub fn to_plain_text(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("Phase connection diagnostics: {}", self.summary));
        if let Ok(timestamp) = self.generated_at.duration_since(UNIX_EPOCH) {
            lines.push(format!("Checked at Unix time: {}", timestamp.as_secs()));
        }
        for check in &self.checks {
            let elapsed = check
                .elapsed_ms
                .map(|value| format!(" ({value} ms)"))
                .unwrap_or_default();
            lines.push(format!(
                "- [{}] {}{}: {}",
                check.status.label(),
                check.title,
                elapsed,
                check.detail
            ));
            if let Some(raw_error) = &check.raw_error {
                if !raw_error.trim().is_empty() {
                    lines.push(format!("  Raw error: {raw_error}"));
                }
            }
            if !check.next_step.trim().is_empty() && check.next_step != "No action needed." {
                lines.push(format!("  Try: {}", check.next_step));
            }
        }
        lines.join("\n")
    }
}

pub fn run(current_build_id: &str, selected_folder: Option<PathBuf>) -> DiagnosticReport {
    let mut checks = Vec::new();
    checks.push(check_dns());
    checks.push(check_phase_tcp());
    checks.push(check_phase_https(current_build_id));
    checks.push(check_plugin_folder(selected_folder));

    let problems = checks
        .iter()
        .filter(|check| check.status == DiagnosticStatus::Problem)
        .count();
    let warnings = checks
        .iter()
        .filter(|check| check.status == DiagnosticStatus::Warning)
        .count();
    let summary = if problems > 0 {
        format!("{problems} connection problem found.")
    } else if warnings > 0 {
        format!("{warnings} thing to check.")
    } else {
        "Everything reachable from this app.".to_owned()
    };

    DiagnosticReport {
        summary,
        generated_at: SystemTime::now(),
        checks,
    }
}

fn check_dns() -> DiagnosticCheck {
    let started = Instant::now();
    match ("phase.motioncore.xyz", 443).to_socket_addrs() {
        Ok(addresses) => {
            let addresses = addresses.collect::<Vec<_>>();
            if addresses.is_empty() {
                return DiagnosticCheck {
                    status: DiagnosticStatus::Problem,
                    title: "Phase server address".to_owned(),
                    detail: "The server name did not return any addresses.".to_owned(),
                    next_step: "Check DNS, VPN, proxy, or try a different network.".to_owned(),
                    raw_error: None,
                    elapsed_ms: Some(started.elapsed().as_millis()),
                };
            }
            let preview = addresses
                .iter()
                .take(2)
                .map(|address| address.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            DiagnosticCheck {
                status: DiagnosticStatus::Good,
                title: "Phase server address".to_owned(),
                detail: format!("Found {preview}."),
                next_step: "No action needed.".to_owned(),
                raw_error: None,
                elapsed_ms: Some(started.elapsed().as_millis()),
            }
        }
        Err(error) => DiagnosticCheck {
            status: DiagnosticStatus::Problem,
            title: "Phase server address".to_owned(),
            detail: format!("Could not find phase.motioncore.xyz: {error}"),
            next_step: "Check DNS, VPN, proxy, or try a different network.".to_owned(),
            raw_error: Some(error.to_string()),
            elapsed_ms: Some(started.elapsed().as_millis()),
        },
    }
}

fn check_phase_tcp() -> DiagnosticCheck {
    let started = Instant::now();
    match ("phase.motioncore.xyz", 443).to_socket_addrs() {
        Ok(addresses) => {
            let mut last_error = None;
            for address in addresses {
                match TcpStream::connect_timeout(&address, std::time::Duration::from_secs(4)) {
                    Ok(_) => {
                        return DiagnosticCheck {
                            status: DiagnosticStatus::Good,
                            title: "Network path to Phase".to_owned(),
                            detail: "The computer can reach Phase on secure web port 443."
                                .to_owned(),
                            next_step: "No action needed.".to_owned(),
                            raw_error: None,
                            elapsed_ms: Some(started.elapsed().as_millis()),
                        };
                    }
                    Err(error) => last_error = Some(format!("{address}: {error}")),
                }
            }
            DiagnosticCheck {
                status: DiagnosticStatus::Problem,
                title: "Network path to Phase".to_owned(),
                detail: "The computer could not open a secure network path to Phase.".to_owned(),
                next_step:
                    "Check firewall, VPN, proxy, or whether phase.motioncore.xyz is blocked."
                        .to_owned(),
                raw_error: last_error,
                elapsed_ms: Some(started.elapsed().as_millis()),
            }
        }
        Err(error) => DiagnosticCheck {
            status: DiagnosticStatus::Problem,
            title: "Network path to Phase".to_owned(),
            detail: "The network path could not be tested because DNS failed.".to_owned(),
            next_step: "Fix DNS first, then run diagnostics again.".to_owned(),
            raw_error: Some(error.to_string()),
            elapsed_ms: Some(started.elapsed().as_millis()),
        },
    }
}

fn check_phase_https(current_build_id: &str) -> DiagnosticCheck {
    let started = Instant::now();
    let plan = verification::VerificationPlan::new(current_build_id);
    match verification::fetch_version(&plan) {
        Ok(version) => DiagnosticCheck {
            status: DiagnosticStatus::Good,
            title: "Secure Phase connection".to_owned(),
            detail: format!(
                "Connected to Phase. Latest plugin: {}.",
                version.latest_version
            ),
            next_step: "No action needed.".to_owned(),
            raw_error: None,
            elapsed_ms: Some(started.elapsed().as_millis()),
        },
        Err(error) => {
            let (detail, next_step) = explain_network_error(&error);
            DiagnosticCheck {
                status: DiagnosticStatus::Problem,
                title: "Secure Phase connection".to_owned(),
                detail,
                next_step,
                raw_error: Some(error),
                elapsed_ms: Some(started.elapsed().as_millis()),
            }
        }
    }
}

fn check_plugin_folder(selected_folder: Option<PathBuf>) -> DiagnosticCheck {
    let started = Instant::now();
    let Some(folder) = selected_folder else {
        return DiagnosticCheck {
            status: DiagnosticStatus::Warning,
            title: "Roblox plugin folder".to_owned(),
            detail: "No install folder is selected.".to_owned(),
            next_step: "Open Folders and choose the Roblox Studio Plugins folder.".to_owned(),
            raw_error: None,
            elapsed_ms: Some(started.elapsed().as_millis()),
        };
    };

    if !folder.exists() {
        return DiagnosticCheck {
            status: DiagnosticStatus::Warning,
            title: "Roblox plugin folder".to_owned(),
            detail: format!("The selected folder does not exist: {}", folder.display()),
            next_step: "Create the folder, choose a different folder, or install once to let Phase create it."
                .to_owned(),
            raw_error: None,
            elapsed_ms: Some(started.elapsed().as_millis()),
        };
    }
    if !folder.is_dir() {
        return DiagnosticCheck {
            status: DiagnosticStatus::Problem,
            title: "Roblox plugin folder".to_owned(),
            detail: format!("The selected path is not a folder: {}", folder.display()),
            next_step: "Open Folders and choose the Roblox Studio Plugins folder.".to_owned(),
            raw_error: None,
            elapsed_ms: Some(started.elapsed().as_millis()),
        };
    }

    let probe = folder.join(".phase-diagnostic-write-test");
    match fs::write(&probe, b"phase") {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            DiagnosticCheck {
                status: DiagnosticStatus::Good,
                title: "Roblox plugin folder".to_owned(),
                detail: format!("Phase can write to {}.", folder.display()),
                next_step: "No action needed.".to_owned(),
                raw_error: None,
                elapsed_ms: Some(started.elapsed().as_millis()),
            }
        }
        Err(error) => DiagnosticCheck {
            status: DiagnosticStatus::Problem,
            title: "Roblox plugin folder".to_owned(),
            detail: format!("Phase cannot write to {}: {error}", folder.display()),
            next_step:
                "Close Roblox Studio, check folder permissions, or choose a local Plugins folder."
                    .to_owned(),
            raw_error: Some(error.to_string()),
            elapsed_ms: Some(started.elapsed().as_millis()),
        },
    }
}

fn explain_network_error(error: &str) -> (String, String) {
    let lower = error.to_ascii_lowercase();
    if lower.contains("certificate")
        || lower.contains("cert")
        || lower.contains("tls")
        || lower.contains("ssl")
        || lower.contains("invalid peer")
    {
        return (
            "The Phase server was reached, but the secure certificate check failed.".to_owned(),
            "Check the computer clock, VPN/proxy, antivirus HTTPS scanning, or captive Wi-Fi login."
                .to_owned(),
        );
    }
    if lower.contains("dns")
        || lower.contains("resolve")
        || lower.contains("lookup")
        || lower.contains("name")
    {
        return (
            format!("The Phase server name could not be resolved: {error}"),
            "Check DNS, VPN, proxy, or try a different network.".to_owned(),
        );
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return (
            format!("The secure Phase connection timed out: {error}"),
            "Check firewall, VPN, proxy, or try again on another network.".to_owned(),
        );
    }
    if lower.contains("refused")
        || lower.contains("reset")
        || lower.contains("connectfail")
        || lower.contains("connection")
    {
        return (
            format!("The secure Phase connection was blocked or interrupted: {error}"),
            "Check firewall, VPN, proxy, and whether phase.motioncore.xyz is allowed.".to_owned(),
        );
    }
    (
        format!("The Phase connection failed: {error}"),
        "Try again, then check VPN/proxy/firewall settings if it keeps happening.".to_owned(),
    )
}
