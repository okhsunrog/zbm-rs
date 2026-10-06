//! PID 1: std/libc only. Never enters Tokio or accesses manager application state.
mod child;
mod console;
mod early_mounts;
mod signals;

use crate::session::{self, Request};
use console::Console;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::{
        fd::AsRawFd,
        unix::{net::UnixDatagram, process::ExitStatusExt},
    },
    process::{Command, ExitStatus},
    time::{Duration, Instant},
};

pub fn log(message: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("/run/zbm-rs/supervisor.log")
    {
        let _ = writeln!(file, "{message}");
    }
    if let Ok(mut file) = OpenOptions::new().write(true).open("/dev/ttyS0") {
        let _ = writeln!(file, "ZBM_SUPERVISOR {message}");
    }
}

pub fn run() -> ! {
    match std::panic::catch_unwind(supervise) {
        Ok(Ok(())) => log("supervisor unexpectedly returned"),
        Ok(Err(error)) => log(&format!("supervisor error: {error}")),
        Err(_) => log("supervisor panic caught"),
    }
    // Never return from PID 1, including bootstrap or recovery failures.
    loop {
        let _ = child::reap(None);
        std::thread::sleep(Duration::from_secs(1));
    }
}

struct CrashPolicy {
    last: Option<Instant>,
    count: u32,
    limit: u32,
}
impl CrashPolicy {
    fn new(limit: u32) -> Self {
        Self {
            last: None,
            count: 0,
            limit,
        }
    }
    fn failed(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(30))
        {
            self.count = 0;
        }
        self.last = Some(now);
        self.count += 1;
        self.count < self.limit
    }
    fn reset(&mut self) {
        *self = Self::new(self.limit);
    }
}

fn supervise() -> io::Result<()> {
    early_mounts::setup()?;
    signals::install()?;
    let mut console = Console::open()?;
    let baseline = console.state()?;
    fs::write("/run/zbm-rs/console-baseline", &baseline)?;
    log(&format!("started pid=1 {baseline}"));
    let mut bootstrap = Command::new("/bin/sh");
    bootstrap
        .arg("/etc/zbm-rs/setup.sh")
        .env("PATH", "/usr/bin:/usr/sbin:/bin:/sbin")
        .env("TERM", "linux")
        .env("LD_LIBRARY_PATH", "/usr/lib");
    let pid = child::spawn(&console, &mut bootstrap, None)?;
    let (status, _) = wait(pid, None)?;
    log(&format!("bootstrap status={status}"));
    console.restore()?;
    let config = loop {
        match crate::config::load(true) {
            Ok(config) => break config,
            Err(error) => {
                log(&format!("invalid image configuration: {error:#}"));
                console.notice(&format!("Invalid image configuration: {error:#}"));
                emergency(&mut console)?;
            }
        }
    };
    let executable = std::env::current_exe()?;
    let mut policy = CrashPolicy::new(config.manager.restart_limit);
    loop {
        let (parent, socket) = UnixDatagram::pair()?;
        parent.set_nonblocking(true)?;
        let mut command = Command::new(&executable);
        command
            .args(["--manager", "--events", "/dev/ttyS0"])
            .env("TERM", "linux")
            .env("LD_LIBRARY_PATH", "/usr/lib")
            .env("PATH", "/usr/bin:/usr/sbin:/bin:/sbin")
            .env(session::FD_ENV, socket.as_raw_fd().to_string());
        let pid = match child::spawn(&console, &mut command, Some(socket.as_raw_fd())) {
            Ok(pid) => pid,
            Err(error) => {
                log(&format!("manager spawn failed: {error}"));
                emergency(&mut console)?;
                continue;
            }
        };
        drop(socket);
        fs::write("/run/zbm-rs/manager.pid", pid.to_string())?;
        log(&format!("manager-start pid={pid}"));
        let (status, request) = wait(pid, Some(&parent))?;
        let _ = fs::remove_file("/run/zbm-rs/manager.pid");
        console.restore()?;
        let restored = console.state()?;
        fs::write("/run/zbm-rs/console-restored", &restored)?;
        log(&format!("console-restored {restored}"));
        log(&format!(
            "manager-exit pid={pid} code={:?} signal={:?} request={request:?}",
            status.code(),
            status.signal()
        ));
        // Requests are accepted only after clean manager termination.
        match (status.success(), request) {
            (true, Some(Request::Restart)) => {
                policy.reset();
                log("controlled-restart");
            }
            (true, Some(Request::EmergencyShell)) => {
                emergency(&mut console)?;
                policy.reset();
            }
            (true, Some(Request::PowerOff | Request::Reboot)) => {
                let how = if request == Some(Request::PowerOff) {
                    libc::RB_POWER_OFF
                } else {
                    libc::RB_AUTOBOOT
                };
                log(&format!("shutdown request={request:?}"));
                unsafe {
                    libc::sync();
                }
                // SAFETY: only PID 1 in this initramfs can reach this system action.
                if unsafe { libc::reboot(how) } < 0 {
                    log(&format!("shutdown failed: {}", io::Error::last_os_error()));
                }
                emergency(&mut console)?;
                policy.reset();
            }
            (_, Some(Request::KexecStarting)) => {
                // A successful kexec never returns to this kernel. Returning is a failed handoff.
                log("kexec-returned; recovery required");
                emergency(&mut console)?;
                policy.reset();
            }
            _ => {
                console.notice(&format!("Manager terminated: {status}. PID 1 is alive."));
                if policy.failed(Instant::now()) {
                    log("crash-restart");
                } else {
                    log("crash-loop");
                    emergency(&mut console)?;
                    policy.reset();
                }
            }
        }
    }
}

fn wait(pid: i32, channel: Option<&UnixDatagram>) -> io::Result<(ExitStatus, Option<Request>)> {
    let mut request = None;
    let mut deadline = None;
    let mut killed = false;
    loop {
        let pending = signals::take();
        for signal in signals::FORWARDED {
            if pending & (1 << signal) != 0 {
                // Signals are logged/forwarded in the main loop, never through a stale reaped PID.
                unsafe {
                    libc::kill(-pid, signal);
                }
                log(&format!("forward signal={signal} pid={pid}"));
            }
        }
        drain(channel, &mut request, &mut deadline)?;
        if let Some(status) = child::reap(Some(pid))? {
            drain(channel, &mut request, &mut deadline)?;
            return Ok((status, request));
        }
        if deadline.is_some_and(|end| Instant::now() >= end) && !killed {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            log(&format!("request-timeout pid={pid}"));
            killed = true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn drain(
    channel: Option<&UnixDatagram>,
    request: &mut Option<Request>,
    deadline: &mut Option<Instant>,
) -> io::Result<()> {
    if let Some(channel) = channel {
        // Bound a faulty manager's message traffic; one intent per generation.
        for _ in 0..8 {
            match session::receive(channel) {
                Ok(Some(value)) if request.is_none() => {
                    *request = Some(value);
                    *deadline = Some(
                        Instant::now()
                            + Duration::from_secs(if value == Request::KexecStarting {
                                30
                            } else {
                                2
                            }),
                    );
                }
                Ok(Some(_)) => log("duplicate supervisor request ignored"),
                Ok(None) => break,
                Err(error) => {
                    log(&format!("protocol error: {error}"));
                    break;
                }
            }
        }
    }
    Ok(())
}

fn emergency(console: &mut Console) -> io::Result<()> {
    console.restore()?;
    console.notice("zbm-rs emergency recovery. Exit the shell to start a new manager.");
    log("emergency-shell");
    let mut command = Command::new("/bin/sh");
    let pid = child::spawn(console, &mut command, None)?;
    wait(pid, None)?;
    console.restore()?;
    log("emergency-return");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rapid_failures_stop_restart_but_a_healthy_interval_resets_budget() {
        let mut policy = CrashPolicy::new(2);
        let now = Instant::now();
        assert!(policy.failed(now));
        assert!(!policy.failed(now + Duration::from_secs(1)));
        assert!(policy.failed(now + Duration::from_secs(32)));
        policy.reset();
        assert!(policy.failed(now + Duration::from_secs(33)));
    }
}
