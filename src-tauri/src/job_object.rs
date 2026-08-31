//! Windows Job Object wrapper: ties the DSH child process to this app's
//! lifetime so that if the launcher exits by ANY path (normal close, crash,
//! or being killed), the OS automatically terminates the child process tree.
//!
//! This prevents leaked `node.exe` / `cmd.exe` processes that would otherwise
//! lock `node.exe` and break future installs/updates.

#![cfg(windows)]

use std::sync::Mutex;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JobObjectExtendedLimitInformation,
};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

/// The single Job Object handle, stored as `usize` (a raw handle value) so it
/// is `Send + Sync` and can live in a `static`. Lazily created on first use.
static JOB_HANDLE: Mutex<Option<usize>> = Mutex::new(None);

/// Create (once) a Job Object that kills its processes when the handle closes.
fn ensure_job() -> Option<usize> {
    let mut guard = JOB_HANDLE.lock().unwrap();
    if let Some(h) = *guard {
        return Some(h);
    }

    unsafe {
        let handle = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
        if handle.is_null() {
            return None;
        }

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        let ok = SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            &mut info as *mut _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            windows_sys::Win32::Foundation::CloseHandle(handle);
            return None;
        }

        let h = handle as usize;
        *guard = Some(h);
        Some(h)
    }
}

/// Assign the given child process (by PID) to the kill-on-close Job Object.
///
/// Returns `true` on success. Call this right after spawning the DSH child.
pub fn assign_process(pid: u32) -> bool {
    let Some(job) = ensure_job() else {
        return false;
    };
    unsafe {
        let handle = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let ok = AssignProcessToJobObject(job as HANDLE, handle);
        windows_sys::Win32::Foundation::CloseHandle(handle);
        ok != 0
    }
}
