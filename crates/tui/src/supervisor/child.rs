use super::{
    console::{Console, check},
    signals,
};
use crate::session;
use std::{
    io,
    os::{
        fd::AsRawFd,
        unix::process::{CommandExt, ExitStatusExt},
    },
    process::{Command, ExitStatus, Stdio},
};

pub fn spawn(console: &Console, command: &mut Command, session_fd: Option<i32>) -> io::Result<i32> {
    command
        .stdin(Stdio::from(console.file.try_clone()?))
        .stdout(Stdio::from(console.file.try_clone()?));
    let console_fd = console.file.as_raw_fd();
    // SAFETY: only async-signal-safe syscalls between fork and exec.
    unsafe {
        command.pre_exec(move || {
            signals::reset_child();
            if libc::setpgid(0, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            check(libc::tcsetpgrp(console_fd, libc::getpid()))?;
            libc::signal(libc::SIGTTOU, libc::SIG_DFL);
            if let Some(fd) = session_fd {
                session::cloexec(fd, false)?;
            }
            Ok(())
        });
    }
    // PID 1 reaps all descendants itself; do not also wait through std::Child.
    Ok(command.spawn()?.id() as i32)
}

/// WNOWAIT keeps the leader's PID reserved while its process group is cleaned up.
pub fn reap(leader: Option<i32>) -> io::Result<Option<ExitStatus>> {
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: valid siginfo storage; peeking prevents PID reuse before cleanup.
        let ret = unsafe {
            libc::waitid(
                libc::P_ALL,
                0,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if ret < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ECHILD) {
                return Ok(None);
            }
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(error);
        }
        let pid = unsafe { info.si_pid() };
        if pid == 0 {
            return Ok(None);
        }
        if leader == Some(pid) {
            // SAFETY: leader is still a zombie and its ID cannot yet be reused.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let mut status = 0;
        if unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let status = ExitStatus::from_raw(status);
        if leader == Some(pid) {
            return Ok(Some(status));
        } else {
            super::log(&format!("reaped pid={pid} status={status}"));
        }
    }
}
