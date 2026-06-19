use crate::verification;
use std::fs;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

#[derive(Clone, Debug)]
pub struct RepairStep {
    pub status: DiagnosticStatus,
    pub title: String,
    pub detail: String,
    pub action: String,
    pub raw_output: Option<String>,
    pub elapsed_ms: Option<u128>,
}

#[derive(Clone, Debug)]
pub struct RepairReport {
    pub summary: String,
    pub likely_cause: String,
    pub generated_at: SystemTime,
    pub steps: Vec<RepairStep>,
    pub final_diagnostics: DiagnosticReport,
}

#[derive(Clone, Debug)]
pub enum RepairEvent {
    Step(RepairStep),
    Finished(RepairReport),
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

impl RepairReport {
    pub fn overall_status(&self) -> DiagnosticStatus {
        self.final_diagnostics.overall_status()
    }

    pub fn to_plain_text(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!("Phase fix assistant: {}", self.summary));
        if let Ok(timestamp) = self.generated_at.duration_since(UNIX_EPOCH) {
            lines.push(format!("Checked at Unix time: {}", timestamp.as_secs()));
        }
        if !self.likely_cause.trim().is_empty() {
            lines.push(format!("Most likely cause: {}", self.likely_cause));
        }
        for step in &self.steps {
            let elapsed = step
                .elapsed_ms
                .map(|value| format!(" ({value} ms)"))
                .unwrap_or_default();
            lines.push(format!(
                "- [{}] {}{}: {}",
                step.status.label(),
                step.title,
                elapsed,
                step.detail
            ));
            if !step.action.trim().is_empty() {
                lines.push(format!("  Action: {}", step.action));
            }
            if let Some(output) = &step.raw_output {
                if !output.trim().is_empty() {
                    lines.push("  Output:".to_owned());
                    for line in output.lines() {
                        lines.push(format!("    {line}"));
                    }
                }
            }
        }
        lines.push(String::new());
        lines.push("Follow-up diagnostics:".to_owned());
        lines.push(self.final_diagnostics.to_plain_text());
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

pub fn run_fix_assistant<F>(
    current_build_id: &str,
    selected_folder: Option<PathBuf>,
    mut on_step: F,
) -> RepairReport
where
    F: FnMut(RepairStep),
{
    let mut steps = Vec::new();
    let mut push_step = |step: RepairStep| {
        on_step(step.clone());
        steps.push(step);
    };

    let baseline = run(current_build_id, selected_folder.clone());
    let phase_https_failed = baseline.checks.iter().any(|check| {
        check.title == "Secure Phase connection" && check.status == DiagnosticStatus::Problem
    });
    let baseline_status = baseline.overall_status();
    push_step(RepairStep {
        status: baseline_status,
        title: "Baseline diagnostics".to_owned(),
        detail: baseline.summary.clone(),
        action: "Ran Phase's normal DNS, TCP, HTTPS, and plugin-folder checks.".to_owned(),
        raw_output: Some(baseline.to_plain_text()),
        elapsed_ms: None,
    });

    let windows_https = check_windows_https(current_build_id);
    let windows_https_failed = windows_https.status == DiagnosticStatus::Problem;
    push_step(windows_https);

    let proxy_environment = check_proxy_environment();
    let proxy_environment_found = proxy_environment.status == DiagnosticStatus::Warning;
    push_step(proxy_environment);

    let windows_user_proxy = check_windows_user_proxy();
    let windows_user_proxy_found = windows_user_proxy
        .raw_output
        .as_deref()
        .is_some_and(windows_user_proxy_configured);
    push_step(windows_user_proxy);

    let winhttp_proxy = check_winhttp_proxy();
    let winhttp_proxy_found = winhttp_proxy
        .raw_output
        .as_deref()
        .is_some_and(winhttp_proxy_configured);
    push_step(winhttp_proxy);

    if phase_https_failed {
        push_step(flush_dns_cache());
    } else {
        push_step(RepairStep {
            status: DiagnosticStatus::Good,
            title: "DNS repair skipped".to_owned(),
            detail: "Phase HTTPS was not failing, so no DNS cache repair was needed.".to_owned(),
            action: "Skipped ipconfig /flushdns.".to_owned(),
            raw_output: None,
            elapsed_ms: None,
        });
    }

    if phase_https_failed && winhttp_proxy_found {
        push_step(reset_winhttp_proxy());
    } else if winhttp_proxy_found {
        push_step(RepairStep {
            status: DiagnosticStatus::Warning,
            title: "Proxy repair skipped".to_owned(),
            detail: "A WinHTTP proxy is configured, but Phase HTTPS was not failing.".to_owned(),
            action: "Skipped netsh winhttp reset proxy.".to_owned(),
            raw_output: None,
            elapsed_ms: None,
        });
    } else {
        push_step(RepairStep {
            status: DiagnosticStatus::Good,
            title: "Proxy repair skipped".to_owned(),
            detail: "No WinHTTP proxy was configured.".to_owned(),
            action: "Skipped netsh winhttp reset proxy.".to_owned(),
            raw_output: None,
            elapsed_ms: None,
        });
    }

    let final_diagnostics = run(current_build_id, selected_folder);
    let final_status = final_diagnostics.overall_status();
    let likely_cause = likely_cause(
        final_status,
        phase_https_failed,
        windows_https_failed,
        proxy_environment_found,
        windows_user_proxy_found,
        winhttp_proxy_found,
    );
    push_step(RepairStep {
        status: final_status,
        title: "Follow-up diagnostics".to_owned(),
        detail: final_diagnostics.summary.clone(),
        action: "Re-ran Phase diagnostics after the safe local repairs.".to_owned(),
        raw_output: Some(final_diagnostics.to_plain_text()),
        elapsed_ms: None,
    });

    let summary = match final_status {
        DiagnosticStatus::Good => "Phase is reachable after the fix assistant.".to_owned(),
        DiagnosticStatus::Warning => {
            "Fix assistant completed with something left to check.".to_owned()
        }
        DiagnosticStatus::Problem => {
            "Fix assistant could not restore the secure Phase connection.".to_owned()
        }
    };

    RepairReport {
        summary,
        likely_cause,
        generated_at: SystemTime::now(),
        steps,
        final_diagnostics,
    }
}

fn check_windows_https(current_build_id: &str) -> RepairStep {
    #[cfg(target_os = "windows")]
    {
        let started = Instant::now();
        let plan = verification::VerificationPlan::new(current_build_id);
        let url = powershell_string(&plan.version_url());
        let script = format!(
            "$ErrorActionPreference='Stop'; \
             $ProgressPreference='SilentlyContinue'; \
             $response = Invoke-WebRequest -UseBasicParsing -TimeoutSec 15 -Uri {url}; \
             \"StatusCode=$($response.StatusCode)\"; \
             \"ContentType=$($response.Headers['Content-Type'])\"; \
             $body = [string]$response.Content; \
             \"BodyPrefix=$($body.Substring(0, [Math]::Min($body.Length, 220)))\""
        );
        let result = run_command(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &script,
            ],
        );
        let output = command_output_text(&result);
        let output_lower = output.to_ascii_lowercase();
        let status = if result.success && output.contains("StatusCode=200") {
            DiagnosticStatus::Good
        } else if output_lower.contains("ssl")
            || output_lower.contains("tls")
            || output_lower.contains("certificate")
            || output_lower.contains("secure channel")
            || output_lower.contains("protocol")
        {
            DiagnosticStatus::Problem
        } else {
            DiagnosticStatus::Warning
        };
        let detail = match status {
            DiagnosticStatus::Good => {
                "Windows can reach the Phase version endpoint over HTTPS.".to_owned()
            }
            DiagnosticStatus::Problem => {
                "Windows also failed the HTTPS request, so the issue is outside the app.".to_owned()
            }
            DiagnosticStatus::Warning => {
                "Windows HTTPS check did not clearly succeed; review the command output.".to_owned()
            }
        };
        RepairStep {
            status,
            title: "Windows HTTPS check".to_owned(),
            detail,
            action: "Ran PowerShell Invoke-WebRequest against the Phase version endpoint."
                .to_owned(),
            raw_output: Some(output),
            elapsed_ms: Some(started.elapsed().as_millis()),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        RepairStep {
            status: DiagnosticStatus::Warning,
            title: "Windows HTTPS check".to_owned(),
            detail: "This repair check is only available on Windows.".to_owned(),
            action: "Skipped PowerShell Invoke-WebRequest.".to_owned(),
            raw_output: None,
            elapsed_ms: None,
        }
    }
}

fn check_proxy_environment() -> RepairStep {
    let started = Instant::now();
    let proxy_keys = [
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "ALL_PROXY",
        "https_proxy",
        "http_proxy",
        "all_proxy",
    ];
    let bypass_keys = ["NO_PROXY", "no_proxy"];
    let mut lines = Vec::new();
    let mut found_proxy = false;

    for key in proxy_keys {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                found_proxy = true;
                lines.push(format!("{key}={}", redact_proxy_value(&value)));
            }
        }
    }
    for key in bypass_keys {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                lines.push(format!("{key}={}", redact_proxy_value(&value)));
            }
        }
    }

    let (status, detail, raw_output) = if found_proxy {
        (
            DiagnosticStatus::Warning,
            "Proxy environment variables are set for this app process.".to_owned(),
            Some(lines.join("\n")),
        )
    } else {
        (
            DiagnosticStatus::Good,
            "No proxy environment variables were found for this app process.".to_owned(),
            if lines.is_empty() {
                None
            } else {
                Some(lines.join("\n"))
            },
        )
    };

    RepairStep {
        status,
        title: "App proxy environment".to_owned(),
        detail,
        action: "Inspected HTTP(S)_PROXY and ALL_PROXY environment variables; no changes made."
            .to_owned(),
        raw_output,
        elapsed_ms: Some(started.elapsed().as_millis()),
    }
}

fn check_windows_user_proxy() -> RepairStep {
    #[cfg(target_os = "windows")]
    {
        let started = Instant::now();
        let script = "$ErrorActionPreference='Stop'; \
             $path = 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings'; \
             $settings = Get-ItemProperty -Path $path; \
             \"ProxyEnable=$($settings.ProxyEnable)\"; \
             \"ProxyServer=$($settings.ProxyServer)\"; \
             \"AutoConfigURL=$($settings.AutoConfigURL)\"";
        let result = run_command(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                script,
            ],
        );
        let output = redact_output_values(&command_output_text(&result));
        let configured = windows_user_proxy_configured(&output);
        let status = if !result.success {
            DiagnosticStatus::Warning
        } else if configured {
            DiagnosticStatus::Warning
        } else {
            DiagnosticStatus::Good
        };
        let detail = if !result.success {
            "Could not read the Windows user proxy settings.".to_owned()
        } else if configured {
            "Windows user proxy or PAC settings are configured.".to_owned()
        } else {
            "Windows user proxy settings are not enabled.".to_owned()
        };

        RepairStep {
            status,
            title: "Windows user proxy".to_owned(),
            detail,
            action: "Read the current user's Windows proxy/PAC settings; no changes made."
                .to_owned(),
            raw_output: Some(output),
            elapsed_ms: Some(started.elapsed().as_millis()),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        RepairStep {
            status: DiagnosticStatus::Warning,
            title: "Windows user proxy".to_owned(),
            detail: "This repair check is only available on Windows.".to_owned(),
            action: "Skipped Windows user proxy inspection.".to_owned(),
            raw_output: None,
            elapsed_ms: None,
        }
    }
}

fn check_winhttp_proxy() -> RepairStep {
    #[cfg(target_os = "windows")]
    {
        let started = Instant::now();
        let result = run_command("netsh", &["winhttp", "show", "proxy"]);
        let output = command_output_text(&result);
        let configured = winhttp_proxy_configured(&output);
        let status = if !result.success {
            DiagnosticStatus::Warning
        } else if configured {
            DiagnosticStatus::Warning
        } else {
            DiagnosticStatus::Good
        };
        let detail = if !result.success {
            "Could not read the WinHTTP proxy setting.".to_owned()
        } else if configured {
            "A WinHTTP proxy is configured on this computer.".to_owned()
        } else {
            "WinHTTP is using direct access with no proxy server.".to_owned()
        };

        RepairStep {
            status,
            title: "Windows proxy setting".to_owned(),
            detail,
            action: "Ran netsh winhttp show proxy.".to_owned(),
            raw_output: Some(output),
            elapsed_ms: Some(started.elapsed().as_millis()),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        RepairStep {
            status: DiagnosticStatus::Warning,
            title: "Windows proxy setting".to_owned(),
            detail: "This repair check is only available on Windows.".to_owned(),
            action: "Skipped netsh winhttp show proxy.".to_owned(),
            raw_output: None,
            elapsed_ms: None,
        }
    }
}

fn flush_dns_cache() -> RepairStep {
    #[cfg(target_os = "windows")]
    {
        let started = Instant::now();
        let result = run_command("ipconfig", &["/flushdns"]);
        let output = command_output_text(&result);
        RepairStep {
            status: if result.success {
                DiagnosticStatus::Good
            } else {
                DiagnosticStatus::Warning
            },
            title: "DNS cache repair".to_owned(),
            detail: if result.success {
                "Windows DNS resolver cache was flushed.".to_owned()
            } else {
                "Windows did not allow the DNS cache flush; review the output.".to_owned()
            },
            action: "Ran ipconfig /flushdns.".to_owned(),
            raw_output: Some(output),
            elapsed_ms: Some(started.elapsed().as_millis()),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        RepairStep {
            status: DiagnosticStatus::Warning,
            title: "DNS cache repair".to_owned(),
            detail: "This repair action is only available on Windows.".to_owned(),
            action: "Skipped ipconfig /flushdns.".to_owned(),
            raw_output: None,
            elapsed_ms: None,
        }
    }
}

fn reset_winhttp_proxy() -> RepairStep {
    #[cfg(target_os = "windows")]
    {
        let started = Instant::now();
        let result = run_command("netsh", &["winhttp", "reset", "proxy"]);
        let output = command_output_text(&result);
        RepairStep {
            status: if result.success {
                DiagnosticStatus::Good
            } else {
                DiagnosticStatus::Warning
            },
            title: "Windows proxy repair".to_owned(),
            detail: if result.success {
                "WinHTTP proxy was reset to direct access.".to_owned()
            } else {
                "WinHTTP proxy reset failed; review the command output.".to_owned()
            },
            action: "Ran netsh winhttp reset proxy because a WinHTTP proxy was configured."
                .to_owned(),
            raw_output: Some(output),
            elapsed_ms: Some(started.elapsed().as_millis()),
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        RepairStep {
            status: DiagnosticStatus::Warning,
            title: "Windows proxy repair".to_owned(),
            detail: "This repair action is only available on Windows.".to_owned(),
            action: "Skipped netsh winhttp reset proxy.".to_owned(),
            raw_output: None,
            elapsed_ms: None,
        }
    }
}

fn likely_cause(
    final_status: DiagnosticStatus,
    phase_https_failed: bool,
    windows_https_failed: bool,
    proxy_environment_found: bool,
    windows_user_proxy_found: bool,
    winhttp_proxy_found: bool,
) -> String {
    if final_status == DiagnosticStatus::Good {
        if windows_user_proxy_found || winhttp_proxy_found {
            return "A local Windows proxy setting was likely interfering with HTTPS.".to_owned();
        }
        return "The safe local repairs cleared the connection problem.".to_owned();
    }

    if proxy_environment_found {
        return "Proxy environment variables are set for the app; remove or correct them, then restart Phase Companion.".to_owned();
    }
    if windows_user_proxy_found {
        return "Windows user proxy or PAC settings are configured; if they are not intentional, turn them off in Windows Proxy settings or allowlist phase.motioncore.xyz.".to_owned();
    }
    if winhttp_proxy_found {
        return "A Windows proxy was configured; if the reset did not help, the network may require an allowlist instead of a direct connection.".to_owned();
    }
    if phase_https_failed && windows_https_failed {
        return "VPN/proxy/firewall, antivirus HTTPS scanning, captive Wi-Fi, or network SSL inspection is returning an invalid TLS response for phase.motioncore.xyz:443.".to_owned();
    }
    if phase_https_failed {
        return "Phase is reachable on port 443, but TLS still fails; this usually points to HTTPS interception, a VPN/proxy, antivirus web shield, or captive Wi-Fi.".to_owned();
    }
    "Review the warning steps above; the secure Phase connection was not the only issue found."
        .to_owned()
}

fn winhttp_proxy_configured(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    !lower.contains("direct access")
        && !lower.contains("no proxy server")
        && (lower.contains("proxy server") || lower.contains("proxy server(s)"))
}

fn windows_user_proxy_configured(output: &str) -> bool {
    let mut proxy_enabled = false;
    let mut pac_configured = false;
    for line in output.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower == "proxyenable=1" {
            proxy_enabled = true;
        }
        if lower.starts_with("autoconfigurl=") {
            let value = trimmed
                .split_once('=')
                .map(|(_, value)| value.trim())
                .unwrap_or_default();
            pac_configured = !value.is_empty();
        }
    }
    proxy_enabled || pac_configured
}

fn redact_proxy_value(value: &str) -> String {
    let mut redacted = value.to_owned();
    if let Some(scheme) = redacted.find("://") {
        let credentials_start = scheme + 3;
        if let Some(at) = redacted[credentials_start..].find('@') {
            redacted.replace_range(credentials_start..credentials_start + at, "redacted");
        }
    }
    redacted
}

fn redact_output_values(output: &str) -> String {
    output
        .lines()
        .map(redact_proxy_value)
        .collect::<Vec<_>>()
        .join("\n")
}

fn powershell_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

struct CommandRun {
    success: bool,
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_command(program: &str, args: &[&str]) -> CommandRun {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(target_os = "windows")]
    command.creation_flags(CREATE_NO_WINDOW);

    match command.output() {
        Ok(output) => CommandRun {
            success: output.status.success(),
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        },
        Err(error) => CommandRun {
            success: false,
            code: None,
            stdout: String::new(),
            stderr: error.to_string(),
        },
    }
}

fn command_output_text(result: &CommandRun) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "Exit: {}",
        result
            .code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "not started".to_owned())
    ));
    if !result.stdout.trim().is_empty() {
        lines.push("Stdout:".to_owned());
        lines.extend(result.stdout.lines().map(|line| line.to_owned()));
    }
    if !result.stderr.trim().is_empty() {
        lines.push("Stderr:".to_owned());
        lines.extend(result.stderr.lines().map(|line| line.to_owned()));
    }
    truncate_output(lines.join("\n"), 5000)
}

fn truncate_output(mut value: String, limit: usize) -> String {
    if value.len() <= limit {
        return value;
    }
    value.truncate(limit);
    value.push_str("\n... output truncated ...");
    value
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
