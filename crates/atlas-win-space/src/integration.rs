//! Reversible per-user Win+E integration with exact ownership checks.
//! Startup may refresh an owned registration in the same executable directory.
use std::{
    io,
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::*,
};

const TARGET: &str =
    r"Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command";
const BACKUP: &str = r"Software\FLDR\Integration\WinE";
const LEGACY_BACKUP: &str = r"Software\Atlas\Integration\WinE";
#[path = "integration_policy.rs"]
mod policy;
const DELEGATE: &str = "DelegateExecute";

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Value {
    kind: u32,
    bytes: Vec<u8>,
}
impl Value {
    fn text(text: &str) -> Self {
        Self {
            kind: REG_SZ,
            bytes: text
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect(),
        }
    }
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn check(code: u32) -> io::Result<()> {
    if code == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}
impl Key {
    fn open(path: &str, create: bool) -> io::Result<Option<Self>> {
        let mut key = 0;
        let name = wide(path);
        let code = unsafe {
            if create {
                RegCreateKeyExW(
                    HKEY_CURRENT_USER,
                    name.as_ptr(),
                    0,
                    null(),
                    REG_OPTION_NON_VOLATILE,
                    KEY_READ | KEY_WRITE,
                    null(),
                    &mut key,
                    null_mut(),
                )
            } else {
                RegOpenKeyExW(HKEY_CURRENT_USER, name.as_ptr(), 0, KEY_READ, &mut key)
            }
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(code)?;
        Ok(Some(Self(key)))
    }
    fn get(&self, name: &str) -> io::Result<Option<Value>> {
        let name = wide(name);
        let (mut kind, mut size) = (0, 0);
        let code = unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                null(),
                &mut kind,
                null_mut(),
                &mut size,
            )
        };
        if code == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check(code)?;
        if size > 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Registry value too large",
            ));
        }
        let mut bytes = vec![0; size as usize];
        check(unsafe {
            RegQueryValueExW(
                self.0,
                name.as_ptr(),
                null(),
                &mut kind,
                bytes.as_mut_ptr(),
                &mut size,
            )
        })?;
        bytes.truncate(size as usize);
        Ok(Some(Value { kind, bytes }))
    }
    fn set(&self, name: &str, value: Option<&Value>) -> io::Result<()> {
        let name = wide(name);
        let code = unsafe {
            match value {
                Some(value) => RegSetValueExW(
                    self.0,
                    name.as_ptr(),
                    0,
                    value.kind,
                    value.bytes.as_ptr(),
                    value.bytes.len() as u32,
                ),
                None => RegDeleteValueW(self.0, name.as_ptr()),
            }
        };
        if code == ERROR_FILE_NOT_FOUND && value.is_none() {
            Ok(())
        } else {
            check(code)
        }
    }
}
fn command(executable: &Path) -> io::Result<Value> {
    let text = executable.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Executable path is not Unicode",
        )
    })?;
    if text.contains(['"', '\0']) || !executable.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid executable path",
        ));
    }
    Ok(Value::text(&format!("\"{text}\"")))
}
struct WindowsRegistry;
impl policy::Registry for WindowsRegistry {
    fn get(&self, slot: policy::Slot, name: &str) -> io::Result<Option<Value>> {
        let path = match slot {
            policy::Slot::Target => TARGET,
            policy::Slot::Current => BACKUP,
            policy::Slot::Legacy => LEGACY_BACKUP,
        };
        Key::open(path, false)?.map_or(Ok(None), |key| key.get(name))
    }
    fn set(&mut self, slot: policy::Slot, name: &str, value: Option<&Value>) -> io::Result<()> {
        let path = match slot {
            policy::Slot::Target => TARGET,
            policy::Slot::Current => BACKUP,
            policy::Slot::Legacy => LEGACY_BACKUP,
        };
        // Opening for write is confined to explicit policy mutations. No key or
        // tree deletion: unrelated registry values and subkeys remain intact.
        Key::open(path, true)?.expect("created registry key").set(name, value)
    }
}
impl Value {
    fn text_value(&self) -> Option<String> {
        if self.kind != REG_SZ || self.bytes.len() % 2 != 0 { return None; }
        let words: Vec<u16> = self.bytes.chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]])).collect();
        let (&0, text) = words.split_last()? else { return None; };
        if text.contains(&0) { return None; }
        String::from_utf16(text).ok()
    }
}
pub fn win_e_enabled() -> io::Result<bool> {
    policy::enabled(&WindowsRegistry)
}
pub fn set_win_e_enabled(enabled: bool) -> io::Result<()> {
    let _guard = super::persistence::ConfigWriteGuard::acquire()?;
    policy::set_enabled(&mut WindowsRegistry, &launch_executable()?, enabled)
}
/// Only refresh an exactly owned handler in the same executable directory.
pub fn refresh_renamed_registration() -> io::Result<bool> {
    let _guard = super::persistence::ConfigWriteGuard::acquire()?;
    policy::refresh(&mut WindowsRegistry, &launch_executable()?)
}
/// MSIX installs live under `WindowsApps`, which Explorer cannot launch
/// directly; the package's execution alias is the stable launchable path.
fn launch_executable() -> io::Result<std::path::PathBuf> {
    if is_packaged() {
        let local = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not set"))?;
        let alias = Path::new(&local).join(r"Microsoft\WindowsApps\FLDR.exe");
        if alias.exists() {
            return Ok(alias);
        }
    }
    std::env::current_exe()
}
pub fn is_packaged() -> bool {
    use windows_sys::Win32::{
        Foundation::ERROR_INSUFFICIENT_BUFFER,
        Storage::Packaging::Appx::GetCurrentPackageFullName,
    };
    let mut len = 0;
    unsafe { GetCurrentPackageFullName(&mut len, std::ptr::null_mut()) == ERROR_INSUFFICIENT_BUFFER }
}
/// Call on an initialized COM apartment after the startup shell work settles.
pub fn free_unused_com_libraries() {
    unsafe { windows::Win32::System::Com::CoFreeUnusedLibraries(); }
}

pub fn double_click_interval_ms() -> u32 {
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() }
}

/// Per-monitor physical drag distances. Keep Win32 calls behind the platform bridge.
pub fn drag_distances(dpi: u32) -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SM_CXDRAG, SM_CYDRAG};
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetSystemMetricsForDpi(index: i32, dpi: u32) -> i32;
    }
    unsafe { (GetSystemMetricsForDpi(SM_CXDRAG, dpi).max(1), GetSystemMetricsForDpi(SM_CYDRAG, dpi).max(1)) }
}

