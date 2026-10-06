//! Load through kexec_file_load only: no silent legacy syscall fallback.
use std::{ffi::CString, fs::File, io, os::fd::AsRawFd};
use zbm_core::boot::BootPlan;

pub struct LoadedKernel;
impl LoadedKernel {
    pub fn load(plan: &BootPlan) -> io::Result<Self> {
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
        Ok(Self)
    }
    pub fn execute(&self) -> io::Result<()> {
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
    }
}
