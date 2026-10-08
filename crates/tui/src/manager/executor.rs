//! Load through kexec_file_load only: no silent legacy syscall fallback.
use std::{ffi::CString, fs::File, io, os::fd::AsRawFd};
use zbm_core::boot::BootPlan;

pub struct LoadedKernel {
    remote: bool,
    _verified: Option<zbm_core::security::VerifiedBootPlan>,
}
impl LoadedKernel {
    pub(crate) fn remote() -> Self {
        Self {
            remote: true,
            _verified: None,
        }
    }
    pub fn load(plan: &BootPlan) -> io::Result<Self> {
        if crate::config::image_enforced() {
            return Err(io::Error::other(
                "Enforced handoff requires the verified broker; path-based loading is forbidden",
            ));
        }
        let kernel = File::open(&plan.kernel)?;
        let initrd = plan.initrd.as_ref().map(File::open).transpose()?;
        let command = CString::new(zbm_core::linux::command_line(&plan.cmdline)?)?;
        let flags: libc::c_ulong = if initrd.is_none() { 4 } else { 0 };
        // SAFETY: live file descriptors, NUL-terminated command line and Linux
        // KEXEC_FILE_NO_INITRAMFS when no initrd exists. Kernel performs verification.
        let result = unsafe {
            libc::syscall(
                libc::SYS_kexec_file_load,
                kernel.as_raw_fd(),
                initrd.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                command.as_bytes_with_nul().len(),
                command.as_ptr(),
                flags,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            remote: false,
            _verified: None,
        })
    }
    pub(crate) fn load_verified(plan: &BootPlan, owned_clone: bool) -> io::Result<Self> {
        let config = crate::config::load(true).map_err(|e| io::Error::other(e.to_string()))?;
        if config.security.mode != crate::config::SecurityMode::Enforce {
            return Err(io::Error::other("Verified broker requires enforced image"));
        }
        let verified = zbm_core::security::Authorization::load_for_plan(
            plan,
            &config.security.target_authorities,
        )?
        .prepare(plan, owned_clone)?;
        let command = CString::new(zbm_core::linux::command_line(verified.arguments())?)?;
        std::fs::write("/run/zbm-rs/kexec-owned", b"verified-broker\n")?;
        let result = unsafe {
            libc::syscall(
                libc::SYS_kexec_file_load,
                verified.kernel().as_raw_fd(),
                verified.initramfs().as_raw_fd(),
                command.as_bytes_with_nul().len(),
                command.as_ptr(),
                0 as libc::c_ulong,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            let _ = std::fs::remove_file("/run/zbm-rs/kexec-owned");
            return Err(error);
        }
        let loaded = Self {
            remote: false,
            _verified: Some(verified),
        };
        crate::tpm::prepared_target(loaded._verified.as_ref().unwrap())?;
        std::fs::write(
            "/run/zbm-rs/verified-boot.json",
            serde_json::to_vec(loaded._verified.as_ref().unwrap().evidence())?,
        )?;
        Ok(loaded)
    }
    pub fn execute(&self) -> io::Result<()> {
        if self.remote {
            return crate::broker::execute().map_err(|e| io::Error::other(e.to_string()));
        }
        unsafe {
            libc::sync();
        }
        if unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_KEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Err(io::Error::other("kexec unexpectedly returned"))
    }
}
impl Drop for LoadedKernel {
    fn drop(&mut self) {
        if self.remote {
            crate::broker::unload();
            return;
        }
        // Discard a loaded kernel on failed preparation/execution.
        unsafe {
            libc::syscall(
                libc::SYS_kexec_file_load,
                -1,
                -1,
                0usize,
                std::ptr::null::<libc::c_char>(),
                1 as libc::c_ulong,
            );
        }
        let _ = std::fs::remove_file("/run/zbm-rs/kexec-owned");
    }
}
