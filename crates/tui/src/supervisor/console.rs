use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};
pub fn check(result: libc::c_int) -> io::Result<()> {
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
pub struct Console {
    pub file: File,
    termios: libc::termios,
    keyboard: i32,
    display: i32,
}
impl Console {
    pub fn open() -> io::Result<Self> {
        // Linux may start init in the kernel's initial session/process group.
        if unsafe { libc::getsid(0) } != unsafe { libc::getpid() } {
            check(unsafe { libc::setsid() })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC)
            .open("/dev/tty1")?;
        let mut this = Self {
            file,
            termios: unsafe { std::mem::zeroed() },
            keyboard: 0,
            display: 0,
        };
        let fd = this.file.as_raw_fd();
        // PID 1 is the persistent session leader. Children use process groups,
        // not new sessions, so their exit cannot hang up the parent's tty FD.
        check(unsafe { libc::ioctl(fd, libc::TIOCSCTTY, 1) })?;
        // SAFETY: all output pointers are aligned and initialized C structures.
        check(unsafe { libc::tcgetattr(fd, &mut this.termios) })?;
        check(unsafe { libc::ioctl(fd, 0x4b44, &mut this.keyboard) })?;
        check(unsafe { libc::ioctl(fd, 0x4b3b, &mut this.display) })?;
        Ok(this)
    }
    pub fn restore(&mut self) -> io::Result<()> {
        let fd = self.file.as_raw_fd();
        check(unsafe { libc::tcsetpgrp(fd, libc::getpgrp()) })?;
        // SAFETY: open console FD and saved Linux terminal/keyboard/display values.
        check(unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &self.termios) })?;
        check(unsafe { libc::ioctl(fd, 0x4b45, self.keyboard) })?;
        check(unsafe { libc::ioctl(fd, 0x4b3a, self.display) })?;
        check(unsafe { libc::ioctl(fd, 0x5606, 1) })?; // VT_ACTIVATE
        check(unsafe { libc::ioctl(fd, 0x5607, 1) })?; // VT_WAITACTIVE
        self.file
            .write_all(b"\x1bc\x1b[?1049l\x1b[?25h\x1b[0m\x1b[2J\x1b[H")?;
        Ok(())
    }
    pub fn state(&self) -> io::Result<String> {
        let mut term: libc::termios = unsafe { std::mem::zeroed() };
        let (mut keyboard, mut display) = (0i32, 0i32);
        let fd = self.file.as_raw_fd();
        // SAFETY: valid output pointers for console ioctls.
        check(unsafe { libc::tcgetattr(fd, &mut term) })?;
        check(unsafe { libc::ioctl(fd, 0x4b44, &mut keyboard) })?;
        check(unsafe { libc::ioctl(fd, 0x4b3b, &mut display) })?;
        Ok(format!(
            "canonical={} echo={} keyboard={} display={}",
            term.c_lflag & libc::ICANON != 0,
            term.c_lflag & libc::ECHO != 0,
            keyboard,
            display
        ))
    }
    pub fn notice(&mut self, message: &str) {
        let _ = writeln!(self.file, "{message}\r");
    }
}
impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
