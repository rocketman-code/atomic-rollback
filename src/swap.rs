//! Filesystem entry swap via renameat2(RENAME_EXCHANGE). Every
//! irreversible operation in the tool (migration steps, rollback,
//! kernel hook) goes through this function.

use std::ffi::CString;
use std::os::fd::AsRawFd;
use std::path::Path;

/// Swap two filesystem entries within the same directory.
/// Uses renameat2(RENAME_EXCHANGE).
/// On btrfs: single transaction (fs/btrfs/inode.c:8276).
/// On vfat: two separate directory entry writes
/// (fs/fat/namei_vfat.c:1097-1100). Each write is a complete
/// 32-byte entry (fs/fat/inode.c:887-906), so partial power
/// loss produces entries with consistent size and cluster fields.
pub fn rename_exchange(dir: &Path, a: &str, b: &str) -> Result<(), String> {
    let dir_fd = std::fs::File::open(dir)
        .map_err(|e| format!("open {}: {e}", dir.display()))?;

    let a = CString::new(a).map_err(|e| format!("invalid name '{a}': {e}"))?;
    let b = CString::new(b).map_err(|e| format!("invalid name '{b}': {e}"))?;

    let ret = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            dir_fd.as_raw_fd(),
            a.as_ptr(),
            dir_fd.as_raw_fd(),
            b.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    };

    if ret != 0 {
        let errno = std::io::Error::last_os_error();
        Err(format!("RENAME_EXCHANGE {}/{} <-> {}/{}: {errno}", dir.display(), a.to_string_lossy(), dir.display(), b.to_string_lossy()))
    } else {
        Ok(())
    }
}
