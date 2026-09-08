use anyhow::{Context, Result, bail};
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::Path;

pub fn rename_no_replace(from: &Path, to: &Path) -> Result<()> {
    let from_c = CString::new(from.as_os_str().as_bytes()).context("source contains NUL")?;
    let to_c = CString::new(to.as_os_str().as_bytes()).context("destination contains NUL")?;
    #[cfg(target_os = "linux")]
    let result = unsafe {
        // musl does not export renameat2; invoke the same kernel operation directly.
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            from_c.as_ptr(),
            libc::AT_FDCWD,
            to_c.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result = unsafe { libc::renamex_np(from_c.as_ptr(), to_c.as_ptr(), libc::RENAME_EXCL) };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let result = -1;
    if result == 0 {
        return Ok(());
    }
    Err(std::io::Error::last_os_error())
        .with_context(|| format!("rename {} -> {}", from.display(), to.display()))
}

pub fn copy_no_replace(from: &Path, to: &Path) -> Result<()> {
    let metadata = fs::metadata(from)?;
    if !metadata.is_file() {
        bail!("copy source is not a regular file: {}", from.display());
    }
    let mut input = OpenOptions::new().read(true).open(from)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(metadata.permissions().mode())
        .open(to)
        .with_context(|| format!("create {}", to.display()))?;
    let result = (|| -> Result<()> {
        let mut buffer = [0_u8; 128 * 1024];
        loop {
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            output.write_all(&buffer[..read])?;
        }
        output.sync_all()?;
        fs::set_permissions(to, metadata.permissions())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(to);
    }
    result
}

pub fn symlink_no_replace(target: &Path, destination: &Path) -> Result<()> {
    symlink(target, destination).with_context(|| format!("symlink {}", destination.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_will_not_replace() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        fs::write(&from, "from").unwrap();
        fs::write(&to, "to").unwrap();
        assert!(rename_no_replace(&from, &to).is_err());
        assert_eq!(fs::read_to_string(to).unwrap(), "to");
        assert!(from.exists());
    }
}
