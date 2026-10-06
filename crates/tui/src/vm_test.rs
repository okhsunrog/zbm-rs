//! Fault injection and diagnostic utilities, only in opt-in disposable images.
use crate::session::{Client, Request};
use std::{
    fs, io,
    os::fd::AsRawFd,
    os::unix::{fs::PermissionsExt, net::UnixDatagram, process::CommandExt},
    process::Command,
};
const SOCKET: &str = "/run/zbm-rs/test-control.sock";
const SUPERVISOR_FAULT: &str = "/run/zbm-rs/supervisor-fault";
pub fn supervisor_fault() -> io::Result<()> {
    if !std::path::Path::new("/etc/zbm-test-ssh").exists() {
        return Ok(());
    }
    let action = match fs::read_to_string(SUPERVISOR_FAULT) {
        Ok(action) => action,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    fs::remove_file(SUPERVISOR_FAULT)?;
    match action.as_str() {
        "error" => Err(io::Error::other("Deliberate VM supervisor error")),
        "panic" => panic!("Deliberate VM supervisor panic"),
        _ => Err(io::Error::other("Unknown supervisor fault")),
    }
}
fn allowed() -> io::Result<()> {
    if !std::path::Path::new("/etc/zbm-test-ssh").exists() {
        return Err(io::Error::other(
            "VM test hooks require the disposable SSH image",
        ));
    }
    Ok(())
}
pub fn utility() -> io::Result<bool> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_none_or(|s| !s.starts_with("--test-")) {
        return Ok(false);
    }
    allowed()?;
    match args.get(1).map(String::as_str) {
        Some("--test-send") => {
            let action = args
                .get(2)
                .ok_or_else(|| io::Error::other("Missing test action"))?;
            let socket = UnixDatagram::unbound()?;
            socket.connect(SOCKET)?;
            socket.send(action.as_bytes())?;
        }
        Some("--test-probe-fd") => {
            let fd = args
                .get(2)
                .and_then(|s| s.parse::<i32>().ok())
                .ok_or_else(|| io::Error::other("Missing fd"))?;
            let open = unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0;
            fs::write("/run/zbm-rs/fd-probe", format!("inherited={open}"))?;
        }
        Some("--test-orphan-helper") | Some("--test-leak-helper") => {
            let leak = args[1] == "--test-leak-helper";
            let (parent, child) = UnixDatagram::pair()?;
            parent.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
            if leak {
                let fd = args
                    .get(2)
                    .and_then(|value| value.parse::<i32>().ok())
                    .ok_or_else(|| io::Error::other("Missing leaked fd"))?;
                if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
                    return Err(io::Error::other("Leaked fd was not inherited"));
                }
                fs::write("/run/zbm-rs/leak.fd", fd.to_string())?;
            }
            // SAFETY: this diagnostic entry never started Tokio or any threads.
            let pid = unsafe { libc::fork() };
            if pid < 0 {
                return Err(io::Error::last_os_error());
            }
            if pid == 0 {
                unsafe {
                    if libc::setsid() < 0 {
                        libc::_exit(24);
                    }
                    if libc::send(child.as_raw_fd(), b"1".as_ptr().cast(), 1, 0) != 1 {
                        libc::_exit(24);
                    }
                    libc::usleep(if leak { 30_000_000 } else { 300_000 });
                    libc::_exit(23);
                }
            }
            // Do not restart the manager until the helper has escaped its group.
            let mut ready = [0];
            if parent.recv(&mut ready)? != 1 || ready != *b"1" {
                return Err(io::Error::other("Helper failed to detach"));
            }
            fs::write(
                if leak {
                    "/run/zbm-rs/leak.pid"
                } else {
                    "/run/zbm-rs/orphan.pid"
                },
                pid.to_string(),
            )?;
        }
        _ => return Err(io::Error::other("Unknown VM diagnostic")),
    }
    Ok(true)
}
pub struct Control(UnixDatagram);
impl Control {
    pub fn open() -> io::Result<Option<Self>> {
        if !std::path::Path::new("/etc/zbm-test-ssh").exists() {
            return Ok(None);
        }
        let _ = fs::remove_file(SOCKET);
        let socket = UnixDatagram::bind(SOCKET)?;
        fs::set_permissions(SOCKET, fs::Permissions::from_mode(0o600))?;
        socket.set_nonblocking(true)?;
        Ok(Some(Self(socket)))
    }
    pub fn poll(&self, channel: Option<&Client>) -> io::Result<Option<Request>> {
        let mut buf = [0; 64];
        let n = match self.0.recv(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e),
        };
        match std::str::from_utf8(&buf[..n]).map_err(io::Error::other)? {
            "abort" => std::process::abort(),
            "segv" => {
                // Rust std's stack-overflow handler consumes a first synthetic SIGSEGV.
                // Select the default action so this hook exercises fatal signal status 11,
                // without invoking undefined memory accesses or changing production code.
                unsafe {
                    libc::signal(libc::SIGSEGV, libc::SIG_DFL);
                    libc::raise(libc::SIGSEGV);
                }
                return Err(io::Error::other("fatal SIGSEGV unexpectedly returned"));
            }
            "panic" => panic!("Deliberate VM manager panic"),
            "supervisor-error" | "supervisor-panic" => {
                crossterm::terminal::enable_raw_mode()?;
                use io::Write;
                std::io::stdout().write_all(b"\x1b[?1049h\x1b[?25l")?;
                record_console_damage()?;
                fs::write(
                    SUPERVISOR_FAULT,
                    if buf[..n] == *b"supervisor-error" {
                        "error"
                    } else {
                        "panic"
                    },
                )?;
            }
            "dirty-abort" => {
                crossterm::terminal::enable_raw_mode()?;
                use io::Write;
                std::io::stdout().write_all(b"\x1b[?1049h\x1b[?25l")?;
                // SAFETY: deliberate damage to this disposable manager's active VT.
                record_console_damage()?;
                std::process::abort();
            }
            "restart" => return Ok(Some(Request::Restart)),
            "kexec-return" => return Ok(Some(Request::KexecStarting)),
            "poweroff" => return Ok(Some(Request::PowerOff)),
            "reboot" => return Ok(Some(Request::Reboot)),
            "orphan" => {
                Command::new(std::env::current_exe()?)
                    .arg("--test-orphan-helper")
                    .status()?;
            }
            "probe-fd" => {
                let fd = channel
                    .ok_or_else(|| io::Error::other("No supervisor"))?
                    .fd();
                Command::new(std::env::current_exe()?)
                    .args(["--test-probe-fd", &fd.to_string()])
                    .status()?;
            }
            "leak-restart" => {
                let fd = channel
                    .ok_or_else(|| io::Error::other("No supervisor"))?
                    .fd();
                let mut cmd = Command::new(std::env::current_exe()?);
                cmd.args(["--test-leak-helper", &fd.to_string()]);
                // SAFETY: intentionally leak only in this test helper's exec.
                unsafe {
                    cmd.pre_exec(move || crate::session::cloexec(fd, false));
                }
                cmd.status()?;
                return Ok(Some(Request::Restart));
            }
            _ => return Err(io::Error::other("Unknown fault injection")),
        }
        Ok(None)
    }
}

fn record_console_damage() -> io::Result<()> {
    for (request, value) in [(0x4b45, 4), (0x4b3a, 1)] {
        // SAFETY: deliberately alter only this test manager's controlling VT.
        if unsafe { libc::ioctl(0, request, value) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let mut term: libc::termios = unsafe { std::mem::zeroed() };
    let (mut keyboard, mut display) = (0i32, 0i32);
    if unsafe { libc::tcgetattr(0, &mut term) } < 0
        || unsafe { libc::ioctl(0, 0x4b44, &mut keyboard) } < 0
        || unsafe { libc::ioctl(0, 0x4b3b, &mut display) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    fs::write(
        "/run/zbm-rs/console-damage.json",
        serde_json::to_vec(&serde_json::json!({
            "canonical": term.c_lflag & libc::ICANON != 0,
            "echo": term.c_lflag & libc::ECHO != 0, "keyboard": keyboard, "display": display
        }))?,
    )?;
    Ok(())
}
