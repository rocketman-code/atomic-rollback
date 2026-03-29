use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::path::Path;

const SYS_RENAMEAT2: libc::c_long = 316;
const RENAME_EXCHANGE: libc::c_uint = 2;

/// Atomically swap two filesystem entries within the same directory.
/// Uses renameat2(RENAME_EXCHANGE), a single syscall.
/// Either both names swap or neither does. No intermediate state.
pub fn atomic_swap(dir: &Path, a: &str, b: &str) -> Result<(), String> {
    let dir_fd = std::fs::File::open(dir)
        .map_err(|e| format!("open {}: {e}", dir.display()))?;

    let a = CString::new(a).map_err(|e| format!("invalid name '{a}': {e}"))?;
    let b = CString::new(b).map_err(|e| format!("invalid name '{b}': {e}"))?;

    let ret = unsafe {
        libc::syscall(
            SYS_RENAMEAT2,
            dir_fd.as_raw_fd(),
            a.as_ptr(),
            dir_fd.as_raw_fd(),
            b.as_ptr(),
            RENAME_EXCHANGE,
        )
    };

    if ret != 0 {
        let errno = std::io::Error::last_os_error();
        Err(format!("RENAME_EXCHANGE {}/{} <-> {}/{}: {errno}", dir.display(), a.to_string_lossy(), dir.display(), b.to_string_lossy()))
    } else {
        Ok(())
    }
}
