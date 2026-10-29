//! Load through kexec_file_load only: no silent legacy syscall fallback.
use std::{ffi::CString, fs::File, io, os::fd::AsRawFd};
use zbm_core::boot::BootPlan;

pub struct LoadedKernel {
    state: LoadedState,
    resume_id: Option<String>,
}

enum LoadedState {
    Remote,
    Local,
    Verified(zbm_core::security::VerifiedBootPlan),
}
impl LoadedKernel {
    pub(crate) fn remote() -> Self {
        Self {
            state: LoadedState::Remote,
            resume_id: None,
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
            state: LoadedState::Local,
            resume_id: None,
        })
    }
    pub(crate) fn load_verified(
        plan: &BootPlan,
        owned_clone: Option<&zbm_core::boot_zfs::ValidatedClone>,
    ) -> io::Result<Self> {
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
            state: LoadedState::Verified(verified),
            resume_id: None,
        };
        loaded.record_evidence()?;
        Ok(loaded)
    }

    fn record_evidence(&self) -> io::Result<()> {
        if let LoadedState::Verified(verified) = &self.state {
            crate::tpm::prepared_target(verified)?;
            std::fs::write(
                "/run/zbm-rs/verified-boot.json",
                serde_json::to_vec(verified.evidence())?,
            )?;
        }
        Ok(())
    }

    pub fn bind_resume(&mut self, pool_guid: u64, plan: &BootPlan) -> anyhow::Result<()> {
        self.resume_id = crate::resume::handoff(pool_guid, plan)?;
        Ok(())
    }
    pub fn execute(&self) -> io::Result<()> {
        if matches!(self.state, LoadedState::Remote) {
            return crate::broker::execute().map_err(|e| io::Error::other(e.to_string()));
        }
        crate::resume::check_handoff(self.resume_id.as_deref())
            .map_err(|e| io::Error::other(e.to_string()))?;
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
        if matches!(self.state, LoadedState::Remote) {
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
