use std::{ffi::CString, fs, io};

fn mount(kind: &str, target: &str, flags: libc::c_ulong) -> io::Result<()> {
    fs::create_dir_all(target)?;
    let source = CString::new(kind)?;
    let target = CString::new(target)?;
    // SAFETY: NUL-terminated strings, no mount data, Linux filesystem names.
    let result = unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            source.as_ptr(),
            flags,
            std::ptr::null(),
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EBUSY) {
            return Err(error);
        }
    }
    Ok(())
}

pub fn setup() -> io::Result<()> {
    mount(
        "proc",
        "/proc",
        libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
    )?;
    mount(
        "sysfs",
        "/sys",
        libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC,
    )?;
    mount("devtmpfs", "/dev", libc::MS_NOSUID)?;
    mount("tmpfs", "/run", libc::MS_NOSUID | libc::MS_NODEV)?;
    mount("tmpfs", "/tmp", libc::MS_NOSUID | libc::MS_NODEV)?;
    mount("devpts", "/dev/pts", libc::MS_NOSUID | libc::MS_NOEXEC)?;
    fs::create_dir_all("/run/zbm-rs")?;
    fs::create_dir_all("/root")?;
    Ok(())
}
