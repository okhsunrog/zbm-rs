//! Bounded, best-effort diagnostics. Logging must never decide whether to boot.
mod persistence;
pub mod recorder;
pub use persistence::persist_failure;
use std::{
    cell::RefCell,
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

pub const EVENTS: &str = "/run/zbm-rs/events.jsonl";
pub const STDERR: &str = "/run/zbm-rs/manager-stderr.log";
pub const PREVIOUS_STDERR: &str = "/run/zbm-rs/previous-manager-stderr.log";
pub const SUPERVISOR: &str = "/run/zbm-rs/supervisor.log";
pub fn continuous() -> bool {
    env!("ZBM_CONTINUOUS_DIAGNOSTICS") == "true"
}

pub fn timestamp() -> String {
    let mut now: libc::timespec = unsafe { std::mem::zeroed() };
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut now) };
    format!("{}.{:03}", now.tv_sec, now.tv_nsec / 1_000_000)
}
const LIMIT: u64 = 512 * 1024;

pub fn open(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
}

fn append(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() as u64 > LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Diagnostic record exceeds log limit",
        ));
    }
    let metadata = file.metadata()?;
    if metadata.is_file() && metadata.len() + bytes.len() as u64 > LIMIT {
        file.set_len(0)?;
    }
    file.write_all(bytes)
}

#[derive(Default)]
pub struct Events {
    file: RefCell<Option<File>>,
    #[cfg(feature = "vm-test")]
    serial: RefCell<Option<File>>,
}

impl Events {
    pub fn open(path: &Path, supervised: bool) -> Self {
        #[cfg(not(feature = "vm-test"))]
        let _ = supervised;
        Self {
            file: RefCell::new(match open(path) {
                Ok(file) => Some(file),
                Err(error) => {
                    eprintln!("Event log {} unavailable: {error}", path.display());
                    None
                }
            }),
            #[cfg(feature = "vm-test")]
            serial: RefCell::new(if supervised {
                // Test-only transport: preserve complete records under bursts.
                // A nonblocking UART can return EAGAIN halfway through JSON.
                OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NOCTTY)
                    .open("/dev/ttyS0")
                    .ok()
            } else {
                None
            }),
        }
    }

    pub fn write(&self, record: &[u8]) {
        let mut file = self.file.borrow_mut();
        if let Some(output) = file.as_mut()
            && let Err(error) = append(output, record)
        {
            *file = None;
            eprintln!("Event log disabled after write failure: {error}");
        }
        #[cfg(feature = "vm-test")]
        {
            let mut serial = self.serial.borrow_mut();
            if let Some(output) = serial.as_mut()
                && let Err(error) = output.write_all(record)
            {
                *serial = None;
                eprintln!("VM serial mirror disabled: {error}");
            }
        }
    }
}

pub fn supervisor(message: &str) {
    if let Ok(mut file) = open(Path::new(SUPERVISOR)) {
        let _ = append(
            &mut file,
            format!("[{}] {message}\n", timestamp()).as_bytes(),
        );
    }
    #[cfg(feature = "vm-test")]
    if let Ok(mut file) = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open("/dev/ttyS0")
    {
        let _ = writeln!(file, "ZBM_SUPERVISOR {message}");
    }
}

/// Only the bounded tail is read. Control characters never reach the console.
pub fn tail(path: &Path, limit: u64) -> io::Result<String> {
    let mut file = File::open(path)?;
    let start = file.metadata()?.len().saturating_sub(limit);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes)
        .chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' {
                ' '
            } else {
                ch
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_diagnostics_are_disabled_without_failing_the_manager() {
        let events = Events::open(Path::new("/dev/full"), false);
        assert!(events.file.borrow().is_some());
        events.write(b"diagnostic\n");
        assert!(events.file.borrow().is_none());
        events.write(b"another event\n");
        let absent = Events::open(Path::new("/nonexistent/zbm/events"), false);
        absent.write(b"event\n");
        assert!(absent.file.borrow().is_none());
    }

    #[test]
    fn diagnostic_tail_is_bounded_and_removes_terminal_controls() {
        let path = std::env::temp_dir().join(format!("zbm-diagnostic-{}", std::process::id()));
        std::fs::write(&path, b"old\nerror:\x1b[31m broken\n").unwrap();
        let result = tail(&path, 20).unwrap();
        assert!(result.len() <= 20);
        assert!(!result.contains('\x1b'));
        assert!(result.contains("broken"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn oversized_records_cannot_grow_the_log() {
        let path =
            std::env::temp_dir().join(format!("zbm-diagnostic-limit-{}", std::process::id()));
        let mut file = open(&path).unwrap();
        append(&mut file, b"previous\n").unwrap();
        assert!(append(&mut file, &vec![b'x'; LIMIT as usize + 1]).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous\n");
        append(&mut file, &vec![b'x'; LIMIT as usize]).unwrap();
        assert_eq!(file.metadata().unwrap().len(), LIMIT);
        std::fs::remove_file(path).unwrap();
    }
}
