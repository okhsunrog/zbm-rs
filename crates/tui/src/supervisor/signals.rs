use std::{
    io,
    sync::atomic::{AtomicU32, Ordering},
};
pub const FORWARDED: [i32; 4] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGQUIT];
static PENDING: AtomicU32 = AtomicU32::new(0);
extern "C" fn handler(signal: i32) {
    if (0..32).contains(&signal) {
        PENDING.fetch_or(1 << signal, Ordering::Relaxed);
    }
}
pub fn install() -> io::Result<()> {
    for signal in [libc::SIGTTIN, libc::SIGTTOU] {
        // PID 1 owns job-control recovery even while the child is foreground.
        unsafe {
            libc::signal(signal, libc::SIG_IGN);
        }
    }
    for signal in FORWARDED.into_iter().chain([libc::SIGCHLD]) {
        // SAFETY: initialized sigaction; handler only touches a lock-free atomic.
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handler as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
        }
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
pub fn take() -> u32 {
    PENDING.swap(0, Ordering::Relaxed)
}
pub fn reset_child() {
    for signal in FORWARDED
        .into_iter()
        .chain([libc::SIGCHLD, libc::SIGTTIN, libc::SIGTTOU])
    {
        // SAFETY: used only in pre_exec; signal is async-signal-safe.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
        }
    }
}
