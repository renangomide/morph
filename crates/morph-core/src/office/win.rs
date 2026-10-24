// SPDX-License-Identifier: GPL-3.0-or-later
//! Small Win32 helpers for the Office engines: registry reads, file versions,
//! process snapshots and kill-on-close job objects. Every function is best effort
//! and never panics; failures return `None` / `false` / an empty list.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
use windows::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY,
    KEY_WOW64_64KEY, REG_SAM_FLAGS, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY,
    RRF_SUBKEY_WOW6464KEY, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
};
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, OpenThread, PROCESS_TERMINATE, ReleaseMutex, ResumeThread,
    THREAD_SUSPEND_RESUME, TerminateProcess, WaitForSingleObject,
};
use windows::core::PCWSTR;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain([0]).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..end])
        .to_string_lossy()
        .into_owned()
}

// ---------------------------------------------------------------- registry

/// Registry root for [`reg_string`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    ClassesRoot,
    LocalMachine,
    CurrentUser,
}

impl Hive {
    fn hkey(self) -> HKEY {
        match self {
            Hive::ClassesRoot => HKEY_CLASSES_ROOT,
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
            Hive::CurrentUser => HKEY_CURRENT_USER,
        }
    }
}

/// Which registry view (64-bit or 32-bit `WOW6432Node`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Native64,
    Wow32,
}

/// A string value (`REG_SZ` / `REG_EXPAND_SZ`, expanded). `value = None` reads the
/// key's default value.
pub fn reg_string(hive: Hive, view: View, subkey: &str, value: Option<&str>) -> Option<String> {
    let key = wide(subkey);
    let name = value.map(wide);
    let name_ptr = name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr()));
    let flags = RRF_RT_REG_SZ
        | RRF_RT_REG_EXPAND_SZ
        | match view {
            View::Native64 => RRF_SUBKEY_WOW6464KEY,
            View::Wow32 => RRF_SUBKEY_WOW6432KEY,
        };
    let mut size: u32 = 0;
    // SAFETY: NUL-terminated strings; first call only asks for the size.
    let status = unsafe {
        RegGetValueW(
            hive.hkey(),
            PCWSTR(key.as_ptr()),
            name_ptr,
            flags,
            None,
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size == 0 || size > 64 * 1024 {
        return None;
    }
    let mut buf = vec![0u16; (size as usize).div_ceil(2) + 1];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: `buf` has room for `bytes` bytes.
    let status = unsafe {
        RegGetValueW(
            hive.hkey(),
            PCWSTR(key.as_ptr()),
            name_ptr,
            flags,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut bytes),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let text = from_wide(&buf);
    (!text.is_empty()).then_some(text)
}

/// Names of a key's direct subkeys (empty if missing).
pub fn reg_subkeys(hive: Hive, view: View, subkey: &str) -> Vec<String> {
    let key = wide(subkey);
    let sam: REG_SAM_FLAGS = KEY_READ
        | match view {
            View::Native64 => KEY_WOW64_64KEY,
            View::Wow32 => KEY_WOW64_32KEY,
        };
    let mut handle = HKEY::default();
    // SAFETY: NUL-terminated key; `handle` receives an open key we close below.
    let status =
        unsafe { RegOpenKeyExW(hive.hkey(), PCWSTR(key.as_ptr()), None, sam, &mut handle) };
    if status != ERROR_SUCCESS {
        return Vec::new();
    }
    let mut names = Vec::new();
    for index in 0..1024u32 {
        let mut buf = [0u16; 256];
        let mut len = buf.len() as u32;
        // SAFETY: `buf` holds `len` UTF-16 units.
        let status = unsafe {
            RegEnumKeyExW(
                handle,
                index,
                Some(windows::core::PWSTR(buf.as_mut_ptr())),
                &mut len,
                None,
                None,
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            break;
        }
        names.push(from_wide(&buf[..len as usize]));
    }
    // SAFETY: `handle` was opened above.
    let _ = unsafe { RegCloseKey(handle) };
    names
}

/// The executable path from a command line such as `"C:\x\WINWORD.EXE" /Automation`
/// or `C:\x\EXCEL.EXE /automation`.
pub fn exe_from_command(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(PathBuf::from(&rest[..end]));
    }
    // Unquoted: everything up to ".exe" (paths may contain spaces).
    let lower = command.to_ascii_lowercase();
    let end = lower.find(".exe").map(|i| i + 4).unwrap_or(command.len());
    let path = command[..end].trim();
    (!path.is_empty()).then(|| PathBuf::from(path))
}

// ---------------------------------------------------------------- versions

/// File version of an executable, e.g. `"16.0.17928.20114"`.
pub fn file_version(path: &Path) -> Option<String> {
    let w = wide_path(path);
    // SAFETY: NUL-terminated path.
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(w.as_ptr()), None) };
    if size == 0 || size > 16 * 1024 * 1024 {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    // SAFETY: `data` is `size` bytes.
    unsafe { GetFileVersionInfoW(PCWSTR(w.as_ptr()), None, size, data.as_mut_ptr().cast()) }
        .ok()?;
    let root = wide("\\");
    let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut len: u32 = 0;
    // SAFETY: `data` holds a version resource; `ptr` points into it on success.
    let found = unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            PCWSTR(root.as_ptr()),
            &mut ptr,
            &mut len,
        )
    };
    if !found.as_bool() || ptr.is_null() || (len as usize) < size_of::<VS_FIXEDFILEINFO>() {
        return None;
    }
    // SAFETY: checked non-null and large enough; read unaligned to be safe.
    let info = unsafe { std::ptr::read_unaligned(ptr.cast::<VS_FIXEDFILEINFO>()) };
    let ms = info.dwFileVersionMS;
    let ls = info.dwFileVersionLS;
    Some(format!(
        "{}.{}.{}.{}",
        ms >> 16,
        ms & 0xffff,
        ls >> 16,
        ls & 0xffff
    ))
}

// ---------------------------------------------------------------- processes

/// Process IDs whose executable name is `exe` (case-insensitive, e.g. `WINWORD.EXE`).
pub fn pids_named(exe: &str) -> Vec<u32> {
    // SAFETY: plain snapshot call; the handle is closed below.
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return Vec::new();
    };
    let mut pids = Vec::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    // SAFETY: `entry.dwSize` is set; the snapshot handle is valid.
    let mut ok = unsafe { Process32FirstW(snap, &mut entry) }.is_ok();
    while ok {
        if from_wide(&entry.szExeFile).eq_ignore_ascii_case(exe) {
            pids.push(entry.th32ProcessID);
        }
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut entry) }.is_ok();
    }
    // SAFETY: handle from CreateToolhelp32Snapshot.
    let _ = unsafe { CloseHandle(snap) };
    pids
}

/// Terminate a process by ID. `false` if it couldn't be opened or killed.
pub fn kill_pid(pid: u32) -> bool {
    // SAFETY: OpenProcess/TerminateProcess/CloseHandle on a handle we own.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) else {
            return false;
        };
        let killed = TerminateProcess(handle, 1).is_ok();
        let _ = CloseHandle(handle);
        killed
    }
}

/// Resume every thread of a process started with `CREATE_SUSPENDED`.
pub fn resume_process(pid: u32) -> bool {
    // SAFETY: snapshot + thread handles are closed below.
    let Ok(snap) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) }) else {
        return false;
    };
    let mut resumed = false;
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    // SAFETY: `entry.dwSize` is set.
    let mut ok = unsafe { Thread32First(snap, &mut entry) }.is_ok();
    while ok {
        if entry.th32OwnerProcessID == pid {
            // SAFETY: handle closed right after use.
            unsafe {
                if let Ok(thread) = OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID) {
                    resumed |= ResumeThread(thread) != u32::MAX;
                    let _ = CloseHandle(thread);
                }
            }
        }
        // SAFETY: as above.
        ok = unsafe { Thread32Next(snap, &mut entry) }.is_ok();
    }
    // SAFETY: handle from CreateToolhelp32Snapshot.
    let _ = unsafe { CloseHandle(snap) };
    resumed
}

/// A job object that kills every process in it when dropped (or on
/// [`KillJob::terminate`]). Child processes (e.g. `soffice.bin` started by
/// `soffice.com`) join the job automatically.
pub struct KillJob {
    handle: HANDLE,
}

// SAFETY: a job handle can be used from any thread.
unsafe impl Send for KillJob {}
// SAFETY: all methods are thread-safe Win32 calls on the handle.
unsafe impl Sync for KillJob {}

impl KillJob {
    pub fn new() -> Option<KillJob> {
        // SAFETY: anonymous job; the handle is owned by the returned value.
        let handle = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.ok()?;
        let job = KillJob { handle };
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `info` is the struct this class expects.
        unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .ok()?;
        Some(job)
    }

    /// Put a process (by raw handle, e.g. `Child::as_raw_handle()`) in the job.
    pub fn assign(&self, process: std::os::windows::io::RawHandle) -> bool {
        // SAFETY: both handles are valid for the call.
        unsafe { AssignProcessToJobObject(self.handle, HANDLE(process)) }.is_ok()
    }

    /// Kill every process in the job now.
    pub fn terminate(&self) {
        // SAFETY: valid job handle.
        let _ = unsafe { TerminateJobObject(self.handle, 1) };
    }

    /// Processes still running in the job.
    pub fn active_processes(&self) -> u32 {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: `info` is the struct this class expects.
        let ok = unsafe {
            QueryInformationJobObject(
                Some(self.handle),
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                None,
            )
        };
        if ok.is_ok() { info.ActiveProcesses } else { 0 }
    }
}

impl Drop for KillJob {
    fn drop(&mut self) {
        // SAFETY: closing the last handle kills the remaining processes
        // (JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE).
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

// ---------------------------------------------------------------- named mutex

/// A held, session-wide named mutex (`Local\…`), shared by every process of
/// this user session. Released on drop, on the thread that took it (Windows
/// also releases it, as "abandoned", if the thread or process dies).
pub struct NamedMutex {
    handle: HANDLE,
}

impl NamedMutex {
    /// Try to take `name` without waiting. `None`: someone else holds it (or
    /// the mutex couldn't be created).
    pub fn try_acquire(name: &str) -> Option<NamedMutex> {
        let w = wide(name);
        // SAFETY: NUL-terminated name; the handle is owned by the result or closed.
        unsafe {
            let handle = CreateMutexW(None, false, PCWSTR(w.as_ptr())).ok()?;
            let wait = WaitForSingleObject(handle, 0);
            if wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED {
                Some(NamedMutex { handle })
            } else {
                let _ = CloseHandle(handle);
                None
            }
        }
    }
}

impl Drop for NamedMutex {
    fn drop(&mut self) {
        // SAFETY: we own the mutex (taken by this thread) and the handle.
        unsafe {
            let _ = ReleaseMutex(self.handle);
            let _ = CloseHandle(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_mutex_is_exclusive_across_threads_and_released() {
        let name = format!("Local\\Morph-Test-{}", uuid::Uuid::new_v4().simple());
        let held = NamedMutex::try_acquire(&name).unwrap();
        let n = name.clone();
        let other = std::thread::spawn(move || NamedMutex::try_acquire(&n).is_some());
        assert!(!other.join().unwrap(), "another thread can't take it");
        drop(held);
        let n = name.clone();
        let other = std::thread::spawn(move || NamedMutex::try_acquire(&n).is_some());
        assert!(other.join().unwrap(), "free after drop");
        // Another process sees it too.
        let held = NamedMutex::try_acquire(&name).unwrap();
        let script = format!(
            "$m = New-Object System.Threading.Mutex($false, '{name}'); \
             if ($m.WaitOne(0)) {{ 'free'; $m.ReleaseMutex() }} else {{ 'held' }}"
        );
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "held");
        drop(held);
    }

    #[test]
    fn exe_paths_from_commands() {
        assert_eq!(
            exe_from_command(
                r#""C:\Program Files\Microsoft Office\root\Office16\WINWORD.EXE" /Automation"#
            ),
            Some(PathBuf::from(
                r"C:\Program Files\Microsoft Office\root\Office16\WINWORD.EXE"
            ))
        );
        assert_eq!(
            exe_from_command(r"C:\Program Files\Office\EXCEL.EXE /automation"),
            Some(PathBuf::from(r"C:\Program Files\Office\EXCEL.EXE"))
        );
        assert_eq!(exe_from_command("  "), None);
    }

    #[test]
    fn registry_and_processes_are_readable() {
        // Always present on Windows.
        let root = reg_string(
            Hive::LocalMachine,
            View::Native64,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            Some("SystemRoot"),
        );
        assert!(root.is_some());
        assert!(!reg_subkeys(Hive::LocalMachine, View::Native64, "SOFTWARE").is_empty());
        assert!(
            reg_string(
                Hive::LocalMachine,
                View::Native64,
                r"SOFTWARE\Nope\Nope",
                None
            )
            .is_none()
        );
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_string_lossy().into_owned();
        assert!(pids_named(&name).contains(&std::process::id()));
        let kernel =
            PathBuf::from(std::env::var("SystemRoot").unwrap()).join(r"System32\kernel32.dll");
        assert!(file_version(&kernel).is_some_and(|v| v.split('.').count() == 4));
    }

    #[test]
    fn kill_job_kills_children() {
        let job = KillJob::new().unwrap();
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "ping -n 30 127.0.0.1 >NUL"])
            .spawn()
            .unwrap();
        use std::os::windows::io::AsRawHandle;
        assert!(job.assign(child.as_raw_handle()));
        assert!(job.active_processes() >= 1);
        job.terminate();
        let status = child.wait().unwrap();
        assert!(!status.success());
    }
}
