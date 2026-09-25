use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const STUDIO_EXE: &str = "RobloxStudioBeta.exe";
const BACKUP_SUFFIX: &str = ".phase-shortcut-router.bak";

// QEngineWidget::processShortcutOverrideEvent contains a guarded modifier test:
//
//   test dword ptr [rsp + event_modifiers], 0xDDFF_FFFF
//
// Changing only the high immediate byte to 0xD9 admits Control through Studio's
// existing shortcut-routing branch. Offsets and PE hashes move between Studio
// builds, so discovery verifies the surrounding control flow and requires one
// unique match inside an executable PE section before either byte is touched.
const STOCK_MASK_BYTE: u8 = 0xdd;
const PATCHED_MASK_BYTE: u8 = 0xd9;
const PATCH_BYTE_INDEX: usize = 39;
const ROUTER_WINDOW_LEN: usize = 96;
const ROUTER_MIN_LEN: usize = 66;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchState {
    Checking,
    Available,
    Enabled,
    Unsupported,
    NotFound,
    Error,
}

#[derive(Clone, Debug)]
pub struct PatchStatus {
    pub state: PatchState,
    pub studio_running: bool,
    pub detail: String,
}

impl PatchStatus {
    pub fn checking() -> Self {
        Self {
            state: PatchState::Checking,
            studio_running: false,
            detail: "Checking Roblox Studio...".to_owned(),
        }
    }

    pub fn can_enable(&self) -> bool {
        self.state == PatchState::Available && !self.studio_running
    }

    pub fn can_disable(&self) -> bool {
        self.state == PatchState::Enabled && !self.studio_running
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchAction {
    Inspect,
    Enable,
    Disable,
}

#[derive(Clone, Debug)]
pub struct PatchOutcome {
    pub status: PatchStatus,
    pub message: String,
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouterEncoding {
    Stock,
    Patched,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RouterSite {
    signature_offset: u64,
    patch_offset: u64,
    encoding: RouterEncoding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RouterScan {
    Found(RouterSite),
    Missing,
    Ambiguous(usize),
}

#[derive(Clone, Copy, Debug)]
struct FileRange {
    offset: u64,
    len: u64,
}

pub fn run(action: PatchAction) -> Result<PatchOutcome, String> {
    if action == PatchAction::Inspect {
        return Ok(PatchOutcome {
            status: inspect(),
            message: "Shortcut status updated.".to_owned(),
            changed: false,
        });
    }

    if studio_is_running() {
        return Err("Close Roblox Studio before changing this toggle.".to_owned());
    }

    let target = discover_studio_executable()?
        .ok_or_else(|| "No local Roblox Studio installation was found.".to_owned())?;

    let changed = match action {
        PatchAction::Enable => enable_target(&target)?,
        PatchAction::Disable => disable_target(&target)?,
        PatchAction::Inspect => unreachable!(),
    };
    let message = match (action, changed) {
        (PatchAction::Enable, true) => {
            "Phase Ctrl shortcuts are on. You can turn them off anytime.".to_owned()
        }
        (PatchAction::Enable, false) => "Phase Ctrl shortcuts are already on.".to_owned(),
        (PatchAction::Disable, true) => {
            "Phase Ctrl shortcuts are off. Studio shortcuts are back to normal.".to_owned()
        }
        (PatchAction::Disable, false) => "Phase Ctrl shortcuts are already off.".to_owned(),
        (PatchAction::Inspect, _) => unreachable!(),
    };

    let status = inspect_target(&target, false);
    let expected = match action {
        PatchAction::Enable => PatchState::Enabled,
        PatchAction::Disable => PatchState::Available,
        PatchAction::Inspect => unreachable!(),
    };
    if status.state != expected {
        return Err(format!(
            "The shortcut toggle changed, but verification failed: {}",
            status.detail
        ));
    }

    Ok(PatchOutcome {
        status,
        message,
        changed,
    })
}

pub fn inspect() -> PatchStatus {
    let running = studio_is_running();
    match discover_studio_executable() {
        Ok(Some(target)) => inspect_target(&target, running),
        Ok(None) => PatchStatus {
            state: PatchState::NotFound,
            studio_running: running,
            detail: "Roblox Studio was not found.".to_owned(),
        },
        Err(error) => PatchStatus {
            state: PatchState::Error,
            studio_running: running,
            detail: error,
        },
    }
}

fn inspect_target(target: &Path, running: bool) -> PatchStatus {
    let base = PatchStatus {
        state: PatchState::Error,
        studio_running: running,
        detail: String::new(),
    };

    let site = match scan_router(target) {
        Ok(RouterScan::Found(site)) => site,
        Ok(RouterScan::Missing) => {
            return PatchStatus {
                state: PatchState::Unsupported,
                detail: "This Studio version isn't supported yet. Nothing was changed.".to_owned(),
                ..base
            };
        }
        Ok(RouterScan::Ambiguous(_)) => {
            return PatchStatus {
                state: PatchState::Unsupported,
                detail: "We couldn't safely identify the shortcut setting. Nothing was changed."
                    .to_owned(),
                ..base
            };
        }
        Err(error) => {
            return PatchStatus {
                detail: error,
                ..base
            };
        }
    };

    if let Err(error) = fs::metadata(target) {
        return PatchStatus {
            detail: format!("Could not inspect Roblox Studio: {error}"),
            ..base
        };
    }

    match site.encoding {
        RouterEncoding::Stock => PatchStatus {
            state: PatchState::Available,
            detail: if running {
                "Off. Close Studio to turn it on.".to_owned()
            } else {
                "Off. Ready to turn on.".to_owned()
            },
            ..base
        },
        RouterEncoding::Patched => PatchStatus {
            state: PatchState::Enabled,
            detail: if running {
                "On. Close Studio to turn it off.".to_owned()
            } else {
                "On. Ready to turn off anytime.".to_owned()
            },
            ..base
        },
    }
}

fn enable_target(target: &Path) -> Result<bool, String> {
    let site = require_router_site(target)?;
    if site.encoding == RouterEncoding::Patched {
        return Ok(false);
    }

    let backup = ensure_backup(target, site)?;
    if let Err(error) = write_router_byte(
        target,
        site.patch_offset,
        STOCK_MASK_BYTE,
        PATCHED_MASK_BYTE,
    ) {
        return Err(format!("Could not turn on Phase shortcuts: {error}"));
    }

    if verify_router_encoding(target, site.patch_offset, RouterEncoding::Patched).is_ok() {
        return Ok(true);
    }

    let rollback = restore_original_byte(target, site.patch_offset, Some(&backup));
    match rollback {
        Ok(()) => Err("The change did not verify, so the original byte was restored.".to_owned()),
        Err(rollback_error) => Err(format!(
            "The change did not verify and rollback failed: {rollback_error}. The backup remains at {}.",
            backup.display()
        )),
    }
}

fn disable_target(target: &Path) -> Result<bool, String> {
    let site = require_router_site(target)?;
    if site.encoding == RouterEncoding::Stock {
        return Ok(false);
    }

    let target_size = fs::metadata(target)
        .map_err(|error| format!("Could not inspect {}: {error}", target.display()))?
        .len();
    let backup = backup_path(target);
    let verified_backup =
        if backup.exists() && verify_backup_for_site(&backup, target_size, site).is_ok() {
            Some(backup.as_path())
        } else {
            None
        };

    restore_original_byte(target, site.patch_offset, verified_backup)?;
    if verify_router_encoding(target, site.patch_offset, RouterEncoding::Stock).is_ok() {
        return Ok(true);
    }

    let recovery = write_router_byte(
        target,
        site.patch_offset,
        STOCK_MASK_BYTE,
        PATCHED_MASK_BYTE,
    );
    match recovery {
        Ok(()) => {
            Err("The restore did not verify, so the known patched byte was reinstated.".to_owned())
        }
        Err(recovery_error) => Err(format!(
            "The restore did not verify and recovery failed: {recovery_error}."
        )),
    }
}

fn restore_original_byte(
    target: &Path,
    patch_offset: u64,
    backup: Option<&Path>,
) -> Result<(), String> {
    let original = if let Some(backup) = backup {
        read_byte(backup, patch_offset)?
    } else {
        // The unique surrounding instruction proves the stock immediate byte.
        // This fallback repairs older Phase patches whose .bak was moved.
        STOCK_MASK_BYTE
    };
    if original != STOCK_MASK_BYTE {
        return Err("The backup does not contain the original shortcut byte.".to_owned());
    }
    write_router_byte(target, patch_offset, PATCHED_MASK_BYTE, original)
}

fn require_router_site(path: &Path) -> Result<RouterSite, String> {
    match scan_router(path)? {
        RouterScan::Found(site) => Ok(site),
        RouterScan::Missing => Err(
            "This Studio build does not contain the recognized shortcut operation. Nothing was changed."
                .to_owned(),
        ),
        RouterScan::Ambiguous(count) => Err(format!(
            "Found {count} possible shortcut operations. Nothing was changed."
        )),
    }
}

fn verify_router_encoding(
    path: &Path,
    expected_offset: u64,
    expected: RouterEncoding,
) -> Result<(), String> {
    match scan_router(path)? {
        RouterScan::Found(site)
            if site.patch_offset == expected_offset && site.encoding == expected =>
        {
            Ok(())
        }
        RouterScan::Found(site) => Err(format!(
            "Shortcut verification moved or changed (offset {:#x}).",
            site.patch_offset
        )),
        RouterScan::Missing => Err("The shortcut signature disappeared after writing.".to_owned()),
        RouterScan::Ambiguous(count) => Err(format!(
            "Shortcut verification became ambiguous ({count} matches)."
        )),
    }
}

fn ensure_backup(target: &Path, site: RouterSite) -> Result<PathBuf, String> {
    let backup = backup_path(target);
    let target_size = fs::metadata(target)
        .map_err(|error| format!("Could not inspect {}: {error}", target.display()))?
        .len();

    if backup.exists() {
        if verify_backup_for_site(&backup, target_size, site).is_ok() {
            return Ok(backup);
        }
        let stale = stale_backup_path(target);
        fs::rename(&backup, &stale).map_err(|error| {
            format!(
                "Could not archive the older shortcut backup at {}: {error}",
                backup.display()
            )
        })?;
    }

    let temp = backup.with_file_name(format!(
        "{}.tmp-{}-{}",
        backup
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("phase-shortcut-router.bak"),
        std::process::id(),
        timestamp_millis()
    ));
    let copy_result = (|| {
        fs::copy(target, &temp)
            .map_err(|error| format!("Could not create the Studio backup: {error}"))?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temp)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("Could not flush the Studio backup: {error}"))?;
        verify_backup_for_site(&temp, target_size, site)?;
        fs::rename(&temp, &backup)
            .map_err(|error| format!("Could not finish the Studio backup: {error}"))?;
        verify_backup_for_site(&backup, target_size, site)
    })();
    if copy_result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    copy_result.map(|_| backup)
}

fn verify_backup_for_site(
    backup: &Path,
    target_size: u64,
    target_site: RouterSite,
) -> Result<(), String> {
    let backup_size = fs::metadata(backup)
        .map_err(|error| format!("Could not inspect {}: {error}", backup.display()))?
        .len();
    if backup_size != target_size {
        return Err("The backup belongs to a different Studio build.".to_owned());
    }
    let window = read_range(backup, target_site.signature_offset, ROUTER_WINDOW_LEN)?;
    if router_encoding_at(&window) != Some(RouterEncoding::Stock) {
        return Err("The backup does not contain the original shortcut operation.".to_owned());
    }
    let backup_patch_offset = target_site.signature_offset + PATCH_BYTE_INDEX as u64;
    if backup_patch_offset != target_site.patch_offset {
        return Err("The backup shortcut offset does not match Studio.".to_owned());
    }
    Ok(())
}

fn scan_router(path: &Path) -> Result<RouterScan, String> {
    let sections = executable_pe_sections(path)?;
    let mut sites = Vec::new();
    for section in sections {
        let len = usize::try_from(section.len)
            .map_err(|_| "Studio's executable section is too large to scan.".to_owned())?;
        let bytes = read_range(path, section.offset, len)?;
        collect_router_sites(&bytes, section.offset, &mut sites);
        if sites.len() > 1 {
            return Ok(RouterScan::Ambiguous(sites.len()));
        }
    }
    Ok(match sites.as_slice() {
        [] => RouterScan::Missing,
        [site] => RouterScan::Found(*site),
        _ => RouterScan::Ambiguous(sites.len()),
    })
}

fn collect_router_sites(bytes: &[u8], file_offset: u64, sites: &mut Vec<RouterSite>) {
    if bytes.len() < ROUTER_MIN_LEN {
        return;
    }
    for index in 0..=bytes.len() - ROUTER_MIN_LEN {
        if bytes[index] != 0x80 || bytes[index + 1] != 0x3d {
            continue;
        }
        let Some(encoding) = router_encoding_at(&bytes[index..]) else {
            continue;
        };
        let signature_offset = file_offset + index as u64;
        sites.push(RouterSite {
            signature_offset,
            patch_offset: signature_offset + PATCH_BYTE_INDEX as u64,
            encoding,
        });
        if sites.len() > 1 {
            return;
        }
    }
}

fn router_encoding_at(bytes: &[u8]) -> Option<RouterEncoding> {
    if bytes.len() < ROUTER_MIN_LEN {
        return None;
    }
    const FIXED: &[(usize, u8)] = &[
        (0, 0x80),
        (1, 0x3d),
        (6, 0x00),
        (7, 0x74),
        (9, 0x80),
        (10, 0xbe),
        (11, 0xb5),
        (12, 0x00),
        (13, 0x00),
        (14, 0x00),
        (15, 0x00),
        (16, 0x74),
        (18, 0x48),
        (19, 0x8d),
        (20, 0x54),
        (21, 0x24),
        (23, 0x48),
        (24, 0x8b),
        (25, 0xcf),
        (26, 0xff),
        (27, 0x15),
        (32, 0xf7),
        (33, 0x44),
        (34, 0x24),
        (36, 0xff),
        (37, 0xff),
        (38, 0xff),
        (40, 0x75),
        (42, 0x48),
        (43, 0x8d),
        (44, 0x15),
        (49, 0x48),
        (50, 0x8b),
        (51, 0xcf),
        (52, 0xe8),
        (57, 0x66),
        (58, 0x83),
        (59, 0x4f),
        (60, 0x12),
        (61, 0x04),
    ];
    if FIXED.iter().any(|(index, value)| bytes[*index] != *value) {
        return None;
    }
    if bytes[22] != bytes[35] {
        return None;
    }

    let first_fail = short_jump_target(9, bytes[8])?;
    let second_fail = short_jump_target(18, bytes[17])?;
    let modifier_fail = short_jump_target(42, bytes[41])?;
    if first_fail != second_fail
        || first_fail != modifier_fail
        || first_fail + 1 >= bytes.len()
        || bytes[first_fail] != 0x32
        || bytes[first_fail + 1] != 0xdb
    {
        return None;
    }

    match bytes[PATCH_BYTE_INDEX] {
        STOCK_MASK_BYTE => Some(RouterEncoding::Stock),
        PATCHED_MASK_BYTE => Some(RouterEncoding::Patched),
        _ => None,
    }
}

fn short_jump_target(next_instruction: usize, displacement: u8) -> Option<usize> {
    let target = next_instruction as isize + (displacement as i8) as isize;
    usize::try_from(target).ok()
}

fn executable_pe_sections(path: &Path) -> Result<Vec<FileRange>, String> {
    let file_len = fs::metadata(path)
        .map_err(|error| format!("Could not inspect {}: {error}", path.display()))?
        .len();
    let dos = read_range(path, 0, 64)?;
    if &dos[0..2] != b"MZ" {
        return Err("Roblox Studio is not a valid Windows executable.".to_owned());
    }
    let pe_offset = read_u32(&dos, 0x3c) as u64;
    let coff = read_range(path, pe_offset, 24)?;
    if &coff[0..4] != b"PE\0\0" {
        return Err("Roblox Studio has an invalid PE header.".to_owned());
    }
    let section_count = read_u16(&coff, 6) as usize;
    let optional_header_size = read_u16(&coff, 20) as u64;
    if section_count == 0 || section_count > 96 {
        return Err("Roblox Studio has an invalid PE section table.".to_owned());
    }
    let table_offset = pe_offset
        .checked_add(24)
        .and_then(|offset| offset.checked_add(optional_header_size))
        .ok_or_else(|| "Roblox Studio's PE section table overflowed.".to_owned())?;
    let table_len = section_count
        .checked_mul(40)
        .ok_or_else(|| "Roblox Studio's PE section table is too large.".to_owned())?;
    let table = read_range(path, table_offset, table_len)?;

    let mut sections = Vec::new();
    for index in 0..section_count {
        let base = index * 40;
        let raw_size = read_u32(&table, base + 16) as u64;
        let raw_offset = read_u32(&table, base + 20) as u64;
        let characteristics = read_u32(&table, base + 36);
        let executable = characteristics & 0x2000_0000 != 0;
        let contains_code = characteristics & 0x0000_0020 != 0;
        if raw_size == 0 || (!executable && !contains_code) {
            continue;
        }
        let end = raw_offset
            .checked_add(raw_size)
            .ok_or_else(|| "Roblox Studio has an invalid executable section.".to_owned())?;
        if end > file_len {
            return Err("Roblox Studio has an out-of-bounds executable section.".to_owned());
        }
        sections.push(FileRange {
            offset: raw_offset,
            len: raw_size,
        });
    }
    if sections.is_empty() {
        return Err("Roblox Studio has no executable PE sections.".to_owned());
    }
    Ok(sections)
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

fn write_router_byte(
    path: &Path,
    offset: u64,
    expected: u8,
    replacement: u8,
) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("Could not open {}: {error}", path.display()))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("Could not seek to Studio's shortcut operation: {error}"))?;
    let mut current = [0u8; 1];
    file.read_exact(&mut current)
        .map_err(|error| format!("Could not read Studio's shortcut operation: {error}"))?;
    if current[0] != expected {
        return Err(format!(
            "Studio changed before the toggle completed (found {:02x}, expected {expected:02x}).",
            current[0]
        ));
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("Could not reseek to Studio's shortcut operation: {error}"))?;
    file.write_all(&[replacement])
        .map_err(|error| format!("Could not write Studio's shortcut operation: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("Could not flush Studio's shortcut change: {error}"))?;
    Ok(())
}

fn read_range(path: &Path, offset: u64, len: usize) -> Result<Vec<u8>, String> {
    let mut file =
        File::open(path).map_err(|error| format!("Could not open {}: {error}", path.display()))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("Could not seek in {}: {error}", path.display()))?;
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes)
        .map_err(|error| format!("Could not read {}: {error}", path.display()))?;
    Ok(bytes)
}

fn read_byte(path: &Path, offset: u64) -> Result<u8, String> {
    Ok(read_range(path, offset, 1)?[0])
}

fn discover_studio_executable() -> Result<Option<PathBuf>, String> {
    let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") else {
        return Ok(None);
    };
    let versions = PathBuf::from(local_app_data)
        .join("Roblox")
        .join("Versions");
    let entries = match fs::read_dir(&versions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "Could not inspect Roblox Studio versions at {}: {error}",
                versions.display()
            ));
        }
    };

    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path().join(STUDIO_EXE);
        if !path.is_file() {
            continue;
        }
        let modified = fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH);
        candidates.push((modified, path));
    }
    candidates.sort_by(|left, right| right.0.cmp(&left.0));
    Ok(candidates.into_iter().next().map(|(_, path)| path))
}

fn backup_path(target: &Path) -> PathBuf {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(STUDIO_EXE);
    target.with_file_name(format!("{file_name}{BACKUP_SUFFIX}"))
}

fn stale_backup_path(target: &Path) -> PathBuf {
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(STUDIO_EXE);
    target.with_file_name(format!(
        "{file_name}.phase-shortcut-router.stale-{}-{}.bak",
        timestamp_millis(),
        std::process::id()
    ))
}

fn timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
fn studio_is_running() -> bool {
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return true;
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = false;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|value| *value == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
                if name.eq_ignore_ascii_case(STUDIO_EXE) {
                    found = true;
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

#[cfg(not(target_os = "windows"))]
fn studio_is_running() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_signature(encoding: RouterEncoding) -> Vec<u8> {
        let mut bytes = vec![0x90; ROUTER_WINDOW_LEN];
        let fixed = [
            (0, 0x80),
            (1, 0x3d),
            (6, 0x00),
            (7, 0x74),
            (8, 55),
            (9, 0x80),
            (10, 0xbe),
            (11, 0xb5),
            (12, 0x00),
            (13, 0x00),
            (14, 0x00),
            (15, 0x00),
            (16, 0x74),
            (17, 46),
            (18, 0x48),
            (19, 0x8d),
            (20, 0x54),
            (21, 0x24),
            (22, 0x48),
            (23, 0x48),
            (24, 0x8b),
            (25, 0xcf),
            (26, 0xff),
            (27, 0x15),
            (32, 0xf7),
            (33, 0x44),
            (34, 0x24),
            (35, 0x48),
            (36, 0xff),
            (37, 0xff),
            (38, 0xff),
            (40, 0x75),
            (41, 22),
            (42, 0x48),
            (43, 0x8d),
            (44, 0x15),
            (49, 0x48),
            (50, 0x8b),
            (51, 0xcf),
            (52, 0xe8),
            (57, 0x66),
            (58, 0x83),
            (59, 0x4f),
            (60, 0x12),
            (61, 0x04),
            (64, 0x32),
            (65, 0xdb),
        ];
        for (index, value) in fixed {
            bytes[index] = value;
        }
        bytes[PATCH_BYTE_INDEX] = match encoding {
            RouterEncoding::Stock => STOCK_MASK_BYTE,
            RouterEncoding::Patched => PATCHED_MASK_BYTE,
        };
        bytes
    }

    #[test]
    fn scanner_accepts_stock_and_patched_router_forms() {
        assert_eq!(
            router_encoding_at(&sample_signature(RouterEncoding::Stock)),
            Some(RouterEncoding::Stock)
        );
        assert_eq!(
            router_encoding_at(&sample_signature(RouterEncoding::Patched)),
            Some(RouterEncoding::Patched)
        );
    }

    #[test]
    fn scanner_requires_the_shared_guard_failure_target() {
        let mut bytes = sample_signature(RouterEncoding::Stock);
        bytes[17] = 45;
        assert_eq!(router_encoding_at(&bytes), None);
    }

    #[test]
    fn scanner_reports_multiple_candidates_instead_of_guessing() {
        let mut bytes = sample_signature(RouterEncoding::Stock);
        bytes.extend_from_slice(&sample_signature(RouterEncoding::Stock));
        let mut sites = Vec::new();
        collect_router_sites(&bytes, 0, &mut sites);
        assert_eq!(sites.len(), 2);
    }

    #[test]
    fn backup_is_adjacent_and_ends_in_bak() {
        let target = Path::new(r"C:\Roblox\version-test\RobloxStudioBeta.exe");
        let backup = backup_path(target);
        assert_eq!(backup.parent(), target.parent());
        assert!(backup.to_string_lossy().ends_with(".bak"));
    }

    #[test]
    #[ignore = "set PHASE_STUDIO_PATCH_FIXTURE to exercise a full real-build round trip"]
    fn compatible_real_build_round_trips_on_an_isolated_copy() {
        let fixture = std::env::var_os("PHASE_STUDIO_PATCH_FIXTURE")
            .map(PathBuf::from)
            .expect("PHASE_STUDIO_PATCH_FIXTURE is required");
        let original = require_router_site(&fixture).unwrap();
        assert_eq!(original.encoding, RouterEncoding::Stock);

        let root = std::env::temp_dir().join(format!(
            "phase-studio-patch-test-{}-{}",
            std::process::id(),
            timestamp_millis()
        ));
        fs::create_dir(&root).unwrap();
        let target = root.join(STUDIO_EXE);
        fs::copy(&fixture, &target).unwrap();

        assert!(enable_target(&target).unwrap());
        assert_eq!(
            require_router_site(&target).unwrap().encoding,
            RouterEncoding::Patched
        );
        assert!(disable_target(&target).unwrap());
        assert_eq!(
            require_router_site(&target).unwrap().encoding,
            RouterEncoding::Stock
        );

        fs::remove_file(backup_path(&target)).unwrap();
        fs::remove_file(&target).unwrap();
        fs::remove_dir(&root).unwrap();
    }
}
