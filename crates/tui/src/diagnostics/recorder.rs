//! Opt-in root recorder, isolated from PID 1 and the UI. Never reads input devices,
//! process memory, environment variables, RPC payloads or passphrase keystrokes.
use super::{EVENTS, STDERR, SUPERVISOR, continuous, supervisor, timestamp};
use std::{
    cell::RefCell,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            net::UnixDatagram,
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const MOUNT: &str = "/run/zbm-rs/recorder-esp";
const FD_ENV: &str = "ZBM_RECORDER_FD";
const BOOTS: usize = 8;
const CHUNKS: usize = 8;
const CHUNK_BYTES: u64 = 512 * 1024;
const TICK: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Control {
    child: Option<(u32, UnixDatagram)>,
    enabled: bool,
    attempted: Option<Instant>,
}
thread_local! { static CONTROL: RefCell<Control> = RefCell::new(Control::default()); }

pub fn start() {
    if continuous() && std::process::id() == 1 {
        CONTROL.with_borrow_mut(|state| state.enabled = true);
        tick();
    }
}

pub fn reaped(pid: i32) {
    CONTROL.with_borrow_mut(|state| {
        if state
            .child
            .as_ref()
            .is_some_and(|(id, _)| *id == pid as u32)
        {
            state.child = None;
            supervisor("diagnostic recorder exited; retry in at most 10 seconds");
        }
    });
}

pub fn tick() {
    CONTROL.with_borrow_mut(|state| {
        if !state.enabled
            || state.child.is_some()
            || state
                .attempted
                .is_some_and(|last| last.elapsed() < Duration::from_secs(10))
        {
            return;
        }
        state.attempted = Some(Instant::now());
        match spawn() {
            Ok(child) => {
                supervisor(&format!("diagnostic recorder started pid={}", child.0));
                state.child = Some(child);
            }
            Err(error) => supervisor(&format!("diagnostic recorder spawn failed: {error}")),
        }
    });
}

fn spawn() -> io::Result<(u32, UnixDatagram)> {
    let (parent, child) = UnixDatagram::pair()?;
    parent.set_read_timeout(Some(Duration::from_secs(2)))?;
    parent.set_write_timeout(Some(Duration::from_millis(100)))?;
    let fd = child.as_raw_fd();
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg("--diagnostic-recorder")
        .env(FD_ENV, fd.to_string())
        .env_remove(crate::session::FD_ENV)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // PID 1 remains single-threaded. The recorder gets no terminal or manager RPC.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            // A crashed recorder must not leave mounted filesystems behind or
            // stack a second FAT mount when PID 1 starts its replacement.
            if libc::unshare(libc::CLONE_NEWNS) < 0
                || libc::mount(
                    std::ptr::null(),
                    c"/".as_ptr(),
                    std::ptr::null(),
                    libc::MS_REC | libc::MS_PRIVATE,
                    std::ptr::null(),
                ) < 0
            {
                return Err(io::Error::last_os_error());
            }
            crate::session::cloexec(fd, false)
        });
    }
    Ok((command.spawn()?.id(), parent))
}

/// A bounded wait in PID 1; storage errors/timeouts never veto a boot or shutdown.
pub fn checkpoint(stop: bool) {
    CONTROL.with_borrow_mut(|state| {
        if stop {
            state.enabled = false;
        }
        if let Some((_, socket)) = &state.child {
            let command = if stop { 2 } else { 1 };
            let result = socket.send(&[command]).and_then(|_| {
                crate::session::wait_acknowledgement(socket, command, Duration::from_secs(2))
            });
            if let Err(error) = result {
                supervisor(&format!("diagnostic checkpoint unavailable: {error}"));
            }
        }
    });
}

pub fn run() -> io::Result<()> {
    if !continuous() || unsafe { libc::geteuid() } != 0 || unsafe { libc::getppid() } != 1 {
        return Err(io::Error::other(
            "Recorder requires an opted-in PID 1 parent",
        ));
    }
    let parent = fs::metadata("/proc/1/exe")?;
    let current = fs::metadata("/proc/self/exe")?;
    if (parent.dev(), parent.ino()) != (current.dev(), current.ino()) {
        return Err(io::Error::other(
            "Recorder parent must be the boot supervisor",
        ));
    }
    let fd = std::env::var(FD_ENV)
        .ok()
        .and_then(|s| s.parse::<i32>().ok())
        .filter(|fd| *fd >= 3)
        .ok_or_else(|| io::Error::other("Missing recorder channel"))?;
    crate::session::cloexec(fd, true)?;
    let channel = unsafe { UnixDatagram::from_raw_fd(fd) };
    channel.set_read_timeout(Some(TICK))?;
    let result = discard_inherited_zfs_mounts().and_then(|()| collect(&channel));
    if let Err(error) = &result {
        supervisor(&format!("diagnostic recorder failed: {error}"));
    }
    result
}

fn discard_inherited_zfs_mounts() -> io::Result<()> {
    // On restart, unshare copies the manager's existing root/keysource mounts.
    // Release them in OUR private namespace so logging cannot pin encryption
    // keys or stop the manager from unmounting/exporting its pool.
    let mounts = fs::read_to_string("/proc/self/mountinfo")?;
    let mut paths = mounts
        .lines()
        .filter_map(|line| {
            let (mount, filesystem) = line.split_once(" - ")?;
            if filesystem.split_whitespace().next()? != "zfs" {
                return None;
            }
            Some(mount.split_whitespace().nth(4)?.to_owned())
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| std::cmp::Reverse(path.len()));
    for path in paths {
        let path = path
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\");
        let path = std::ffi::CString::new(path)?;
        if unsafe { libc::umount2(path.as_ptr(), libc::MNT_DETACH) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn collect(channel: &UnixDatagram) -> io::Result<()> {
    let partuuid = env!("ZBM_DIAGNOSTIC_ESP_PARTUUID");
    let mut last_error = None;
    // The ESP may appear late. Stay responsive to shutdown while waiting for it.
    let mounted = loop {
        match super::persistence::mount_esp(partuuid, MOUNT) {
            Ok(mounted) => break mounted,
            Err(error) => {
                if last_error.is_none_or(|last: Instant| last.elapsed() >= Duration::from_secs(10))
                {
                    supervisor(&format!("diagnostic ESP not ready: {error}"));
                    last_error = Some(Instant::now());
                }
                if let Some(command) = receive(channel)? {
                    channel.send(&[0])?;
                    if command == 2 {
                        return Ok(());
                    }
                }
            }
        }
    };
    let root = Path::new(MOUNT).join("EFI/zbm-rs/diagnostics");
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let directory = new_boot(&root, boot_id.trim())?;
    let mut log = Ring::open(&directory)?;
    let mut metadata = format!(
        "zbm-rs continuous diagnostic v1\nboot_id={}\nversion={}\nstart_uptime={}\nThese logs are not authenticated boot evidence.\n",
        boot_id.trim(),
        env!("CARGO_PKG_VERSION"),
        timestamp()
    );
    for path in [
        "/proc/version",
        "/proc/cmdline",
        "/etc/zbm-rs/config.json",
        "/etc/zbm-rs/build-info.json",
        "/sys/class/dmi/id/product_name",
        "/sys/class/dmi/id/bios_version",
        "/sys/class/dmi/id/bios_date",
        "/sys/devices/system/cpu/cpu0/microcode/version",
        "/sys/devices/system/cpu/cpu0/microcode/processor_flags",
        "/sys/module/efi_pstore/parameters/pstore_disable",
        "/sys/module/pstore/parameters/backend",
        "/proc/sys/kernel/panic_print",
    ] {
        metadata.push_str(&format!(
            "\n{path}\n{}\n",
            String::from_utf8_lossy(&bounded_read(path, 16 * 1024))
        ));
    }
    // Kept separately so a long session cannot rotate away its build identity.
    let mut metadata_file = File::create(directory.join("metadata.log"))?;
    metadata_file.write_all(metadata.as_bytes())?;
    metadata_file.sync_all()?;
    drop(metadata_file);
    File::open(&directory)?.sync_all()?;
    log.record("recorder-start", metadata.as_bytes())?;
    if let Ok(entries) = fs::read_dir("/sys/fs/pstore") {
        for entry in entries.flatten().take(32) {
            log.record(
                &format!("pstore/{}", entry.file_name().to_string_lossy()),
                &bounded_read(entry.path(), 64 * 1024),
            )?;
        }
    }
    let mut sources = [
        Source::new(SUPERVISOR),
        Source::new(STDERR),
        Source::new(EVENTS),
    ];
    let mut kernel = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/dev/kmsg")
        .ok();
    if kernel.is_none() {
        log.record("recorder", b"kernel log unavailable")?;
    }
    let mut health_at = Instant::now() - Duration::from_secs(2);
    supervisor(&format!(
        "continuous diagnostics active: {} (250ms sync, 8 boots x 4MiB)",
        directory.display()
    ));
    loop {
        for source in &mut sources {
            source.drain(&mut log)?;
        }
        if let Some(kernel) = &mut kernel {
            drain_kernel(kernel, &mut log)?;
        }
        if health_at.elapsed() >= Duration::from_secs(2) {
            log.record("health", health().as_bytes())?;
            health_at = Instant::now();
        }
        // Record every flush cycle, even while the UI is idle. A health sample
        // alone only bounds the last durable observation to two seconds.
        log.record("recorder-tick", b"")?;
        log.sync()?;
        if let Some(command) = receive(channel)? {
            // Collect again so a shutdown marker preceding the request is durable.
            for source in &mut sources {
                source.drain(&mut log)?;
            }
            if let Some(kernel) = &mut kernel {
                drain_kernel(kernel, &mut log)?;
            }
            log.record(
                "checkpoint",
                if command == 2 {
                    b"stop and unmount"
                } else {
                    b"flush"
                },
            )?;
            log.sync()?;
            if command == 2 {
                drop(log);
                drop(mounted);
                channel.send(&[command])?;
                return Ok(());
            }
            channel.send(&[command])?;
        }
    }
}

fn receive(channel: &UnixDatagram) -> io::Result<Option<u8>> {
    let mut bytes = [0u8; 2];
    match channel.recv(&mut bytes) {
        Ok(1) if matches!(bytes[0], 1 | 2) => Ok(Some(bytes[0])),
        Ok(_) => Err(io::Error::other("Invalid recorder command")),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn numbered(name: &str, prefix: &str, suffix: &str) -> Option<u64> {
    let digits = name.strip_prefix(prefix)?.strip_suffix(suffix)?;
    if digits.len() != 16 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn new_boot(root: &Path, boot_id: &str) -> io::Result<PathBuf> {
    // Fixed numeric names keep retention independent of the firmware clock.
    fs::create_dir_all(root)?;
    let mut boots = fs::read_dir(root)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let id = numbered(entry.file_name().to_str()?, "boot-", "")?;
            entry
                .file_type()
                .ok()?
                .is_dir()
                .then_some((id, entry.path()))
        })
        .collect::<Vec<_>>();
    boots.sort();
    // Resume the same directory if the recorder itself was restarted.
    if let Some((_, path)) = boots.last()
        && fs::read_to_string(path.join("boot-id")).ok().as_deref() == Some(boot_id)
    {
        return Ok(path.clone());
    }
    let next = boots.last().map_or(0, |(id, _)| id + 1);
    for (_, path) in boots.iter().take(boots.len().saturating_sub(BOOTS - 1)) {
        fs::remove_dir_all(path)?;
    }
    let path = root.join(format!("boot-{next:016}"));
    fs::create_dir(&path)?;
    let mut id = File::create(path.join("boot-id"))?;
    id.write_all(boot_id.as_bytes())?;
    id.sync_all()?;
    File::open(&path)?.sync_all()?;
    File::open(root)?.sync_all()?;
    Ok(path)
}

struct Ring {
    directory: PathBuf,
    file: File,
    sequence: u64,
    bytes: u64,
}
impl Ring {
    fn open(directory: &Path) -> io::Result<Self> {
        let sequence = fs::read_dir(directory)?
            .filter_map(Result::ok)
            .filter_map(|e| numbered(e.file_name().to_str()?, "trace-", ".log"))
            .max()
            .unwrap_or(0);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(directory.join(format!("trace-{sequence:016}.log")))?;
        let bytes = file.metadata()?.len();
        File::open(directory)?.sync_all()?;
        Ok(Self {
            directory: directory.into(),
            file,
            sequence,
            bytes,
        })
    }
    fn write(&mut self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            if self.bytes >= CHUNK_BYTES {
                self.file.sync_all()?;
                self.sequence += 1;
                self.file = OpenOptions::new().write(true).create_new(true).open(
                    self.directory
                        .join(format!("trace-{:016}.log", self.sequence)),
                )?;
                self.bytes = 0;
                if self.sequence >= CHUNKS as u64 {
                    let old = self
                        .directory
                        .join(format!("trace-{:016}.log", self.sequence - CHUNKS as u64));
                    match fs::remove_file(old) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e),
                    }
                }
                File::open(&self.directory)?.sync_all()?;
            }
            let count = data.len().min((CHUNK_BYTES - self.bytes) as usize);
            self.file.write_all(&data[..count])?;
            self.bytes += count as u64;
            data = &data[count..];
        }
        Ok(())
    }
    fn record(&mut self, source: &str, data: &[u8]) -> io::Result<()> {
        self.write(format!("\n[{}] {source}\n", timestamp()).as_bytes())?;
        self.write(data)?;
        self.write(b"\n")
    }
    fn sync(&self) -> io::Result<()> {
        self.file.sync_all()
    }
}

struct Source {
    path: &'static str,
    inode: u64,
    offset: u64,
    pending: Vec<u8>,
}
impl Source {
    fn new(path: &'static str) -> Self {
        Self {
            path,
            inode: 0,
            offset: 0,
            pending: Vec::new(),
        }
    }
    fn drain(&mut self, log: &mut Ring) -> io::Result<()> {
        let Ok(mut file) = File::open(self.path) else {
            return Ok(());
        };
        let metadata = file.metadata()?;
        if metadata.ino() != self.inode || metadata.len() < self.offset {
            self.inode = metadata.ino();
            self.offset = 0;
            self.pending.clear();
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        file.take(64 * 1024).read_to_end(&mut bytes)?;
        if !bytes.is_empty() {
            if self.path == EVENTS {
                self.pending.extend_from_slice(&bytes);
                while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                    let line = self.pending.drain(..=end).collect::<Vec<_>>();
                    if let Some(safe) = persistent_event(&line) {
                        log.record(self.path, &safe)?;
                    }
                }
                if self.pending.len() > 512 * 1024 {
                    self.pending.clear();
                    log.record("recorder", b"oversized event omitted")?;
                }
            } else {
                log.record(self.path, &bytes)?;
            }
            self.offset += bytes.len() as u64;
        }
        Ok(())
    }
}

fn persistent_event(line: &[u8]) -> Option<Vec<u8>> {
    // Search text could contain a secret if a UI bug unexpectedly closed the
    // unlock panel. Keep it in RAM for rendering, never in the disk recorder.
    let mut value: serde_json::Value = serde_json::from_slice(line).ok()?;
    if let Some(ui) = value
        .get_mut("ui")
        .and_then(serde_json::Value::as_object_mut)
    {
        ui.remove("filter");
    }
    serde_json::to_vec(&value).ok()
}

fn drain_kernel(kernel: &mut File, log: &mut Ring) -> io::Result<()> {
    let mut bytes = [0u8; 8192];
    for _ in 0..256 {
        match kernel.read(&mut bytes) {
            Ok(0) => break,
            Ok(size) => log.record("kernel", &bytes[..size])?,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.raw_os_error() == Some(libc::EPIPE) => {
                log.record("kernel", b"kernel ring overrun")?
            }
            Err(error) => {
                log.record("kernel", error.to_string().as_bytes())?;
                break;
            }
        }
    }
    Ok(())
}

fn bounded_read(path: impl AsRef<Path>, limit: u64) -> Vec<u8> {
    let mut data = Vec::new();
    if let Ok(file) = File::open(path) {
        let _ = file.take(limit).read_to_end(&mut data);
    }
    data
}

fn health() -> String {
    let mut result = format!("heartbeat uptime={}\n", timestamp());
    for path in ["/proc/loadavg", "/proc/meminfo", "/proc/sys/kernel/tainted"] {
        result.push_str(&format!(
            "{path}: {}\n",
            String::from_utf8_lossy(&bounded_read(path, 4096))
        ));
    }
    for role in ["manager", "broker"] {
        let pid = String::from_utf8_lossy(&bounded_read(format!("/run/zbm-rs/{role}.pid"), 32))
            .trim()
            .parse::<u32>();
        if let Ok(pid) = pid {
            for field in ["status", "wchan"] {
                let path = format!("/proc/{pid}/{field}");
                result.push_str(&format!(
                    "{role} {path}: {}\n",
                    String::from_utf8_lossy(&bounded_read(&path, 4096))
                ));
            }
        }
    }
    if let Ok(devices) = fs::read_dir("/sys/class/hwmon") {
        for device in devices.flatten().take(32) {
            if let Ok(fields) = fs::read_dir(device.path()) {
                for field in fields.flatten().take(128) {
                    let name = field.file_name();
                    let name = name.to_string_lossy();
                    if name == "name"
                        || (name.starts_with("temp")
                            && ["_input", "_crit", "_label", "_alarm"]
                                .iter()
                                .any(|suffix| name.ends_with(suffix)))
                    {
                        result.push_str(&format!(
                            "{}: {}",
                            field.path().display(),
                            String::from_utf8_lossy(&bounded_read(field.path(), 256))
                        ));
                    }
                }
            }
        }
    }
    for (root, fields) in [
        (
            "/sys/class/thermal",
            &["type", "temp", "mode", "cur_state"] as &[_],
        ),
        (
            "/sys/class/power_supply",
            &[
                "type",
                "status",
                "online",
                "capacity",
                "voltage_now",
                "current_now",
                "power_now",
                "temp",
            ],
        ),
        (
            "/sys/class/watchdog",
            &[
                "identity",
                "state",
                "status",
                "bootstatus",
                "timeout",
                "timeleft",
            ],
        ),
    ] {
        if let Ok(entries) = fs::read_dir(root) {
            for entry in entries.flatten().take(64) {
                for field in fields {
                    let path = entry.path().join(field);
                    let data = bounded_read(&path, 256);
                    if !data.is_empty() {
                        result.push_str(&format!(
                            "{}: {}",
                            path.display(),
                            String::from_utf8_lossy(&data)
                        ));
                    }
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persisted_events_exclude_search_text_and_malformed_partial_records() {
        let event = br#"{"event":"search","ui":{"panel":"None","filter":"synthetic-secret"},"state":{"scanning":false}}"#;
        let safe = String::from_utf8(persistent_event(event).unwrap()).unwrap();
        assert!(!safe.contains("synthetic-secret"));
        assert!(safe.contains("scanning"));
        assert!(persistent_event(b"synthetic-secret").is_none());
        assert!(persistent_event(&event[..event.len() / 2]).is_none());
    }
    #[test]
    fn retention_resumes_current_boot_and_never_removes_unrelated_paths() {
        let root =
            std::env::temp_dir().join(format!("zbm-recorder-retention-{}", std::process::id()));
        fs::create_dir_all(root.join("owner-files")).unwrap();
        fs::write(root.join("owner-files/keep"), b"keep").unwrap();
        for id in 0..12 {
            new_boot(&root, &id.to_string()).unwrap();
        }
        let latest = new_boot(&root, "11").unwrap();
        assert_eq!(latest.file_name().unwrap(), "boot-0000000000000011");
        assert_eq!(fs::read_dir(&root).unwrap().count(), BOOTS + 1);
        let mut log = Ring::open(&latest).unwrap();
        log.write(&vec![b'x'; (CHUNK_BYTES as usize) * (CHUNKS + 2) + 10])
            .unwrap();
        log.sync().unwrap();
        drop(log);
        let files = fs::read_dir(&latest)
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        assert_eq!(files.len(), CHUNKS + 1);
        assert!(
            files
                .iter()
                .all(|e| e.metadata().unwrap().len() <= CHUNK_BYTES)
        );
        assert_eq!(fs::read(root.join("owner-files/keep")).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }
}
