//! PID-1-only, failure-time export to the explicitly configured FAT partition.
use super::{PREVIOUS_STDERR, STDERR, SUPERVISOR, supervisor, tail};
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, OpenOptionsExt},
    },
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

const MOUNT: &str = "/run/zbm-rs/diagnostic-esp";
const REPORT: &str = "EFI/zbm-rs/last-failure.log";
const MAX_REPORT: usize = 128 * 1024;

pub fn persist_failure() {
    // The build host validates this same schema. Pinning the identifier here
    // also retains diagnostics when runtime configuration validation fails.
    let partuuid = env!("ZBM_DIAGNOSTIC_ESP_PARTUUID");
    if partuuid.is_empty() {
        supervisor("ESP failure report disabled (diagnostics.esp_partuuid is null)");
        return;
    }
    let result = save(partuuid);
    match result {
        Ok(()) => supervisor(&format!(
            "Failure report saved and synced: PARTUUID={partuuid} /{REPORT}"
        )),
        Err(error) => supervisor(&format!("Cannot save ESP failure report: {error}")),
    }
}

struct Mounted;
impl Drop for Mounted {
    fn drop(&mut self) {
        let target = CString::new(MOUNT).unwrap();
        // Never detach a still-busy filesystem or alter a foreign mount.
        if unsafe { libc::umount(target.as_ptr()) } < 0 {
            supervisor(&format!(
                "Diagnostic ESP unmount failed: {}",
                io::Error::last_os_error()
            ));
        }
    }
}

fn save(partuuid: &str) -> io::Result<()> {
    let device = fs::canonicalize(format!(
        "/dev/disk/by-partuuid/{}",
        partuuid.to_ascii_lowercase()
    ))?;
    if !fs::metadata(&device)?.file_type().is_block_device() {
        return Err(io::Error::other(
            "Configured diagnostic ESP is not a block device",
        ));
    }
    fs::create_dir_all(MOUNT)?;
    let source = CString::new(device.as_os_str().as_encoded_bytes())?;
    let target = CString::new(MOUNT)?;
    let kind = c"vfat";
    // FAT has no Unix permissions. Only PID 1's root-owned mount can write it.
    let flags = libc::MS_NODEV | libc::MS_NOSUID | libc::MS_NOEXEC;
    if unsafe {
        libc::mount(
            source.as_ptr(),
            target.as_ptr(),
            kind.as_ptr(),
            flags,
            c"uid=0,gid=0,umask=0077,iocharset=iso8859-1"
                .as_ptr()
                .cast(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let _mounted = Mounted;
    write_report(Path::new(MOUNT), &report())?;
    let directory = File::open(MOUNT)?;
    if unsafe { libc::syncfs(directory.as_raw_fd()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn report() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default();
    let mut report = format!(
        "zbm-rs {} failure report\nunix_time={seconds}\nboot_id={}\nThis is diagnostic data, not authenticated boot evidence.\n",
        env!("CARGO_PKG_VERSION"),
        boot_id.trim()
    );
    for path in [PREVIOUS_STDERR, STDERR, SUPERVISOR] {
        report.push_str(&format!("\n--- {path} ---\n"));
        match tail(Path::new(path), 16 * 1024) {
            Ok(text) => report.push_str(&text),
            Err(error) => report.push_str(&format!("Unavailable: {error}\n")),
        }
    }
    report.push_str("\n--- kernel log (bounded tail) ---\n");
    // Nonblocking reads and an iteration limit: never follow the live log.
    match kernel_tail() {
        Ok(text) => report.push_str(&text),
        Err(error) => report.push_str(&format!("Unavailable: {error}\n")),
    }
    let mut end = report.len().min(MAX_REPORT);
    while !report.is_char_boundary(end) {
        end -= 1;
    }
    report.truncate(end);
    report
}

fn kernel_tail() -> io::Result<String> {
    let mut source = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/dev/kmsg")?;
    let mut output = Vec::new();
    let mut record = [0u8; 8192];
    for _ in 0..4096 {
        match source.read(&mut record) {
            Ok(0) => break,
            Ok(size) => {
                output.extend_from_slice(&record[..size]);
                if output.len() > 64 * 1024 {
                    output.drain(..output.len() - 64 * 1024);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.raw_os_error() == Some(libc::EPIPE) => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(String::from_utf8_lossy(&output)
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

fn write_report(root: &Path, report: &str) -> io::Result<()> {
    let final_path = root.join(REPORT);
    fs::create_dir_all(final_path.parent().unwrap())?;
    let temporary = final_path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temporary)?;
    file.write_all(report.as_bytes())?;
    file.sync_all()?;
    fs::rename(temporary, final_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn report_replaces_only_its_fixed_file_and_is_bounded() {
        let root =
            std::env::temp_dir().join(format!("zbm-persistent-diagnostics-{}", std::process::id()));
        fs::create_dir_all(root.join("EFI/zbm-rs")).unwrap();
        fs::write(root.join("EFI/zbm-rs/loader.EFI"), b"unchanged").unwrap();
        write_report(&root, "previous failure").unwrap();
        let report = report();
        assert!(report.len() <= MAX_REPORT);
        write_report(&root, &report).unwrap();
        assert_eq!(fs::read_to_string(root.join(REPORT)).unwrap(), report);
        assert_eq!(
            fs::read(root.join("EFI/zbm-rs/loader.EFI")).unwrap(),
            b"unchanged"
        );
        assert!(!root.join("EFI/zbm-rs/last-failure.tmp").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
