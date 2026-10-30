//! Native Linux swap header handling on verified dedicated 4 KiB-page devices.
//! The first 1024 bytes are Linux swap bootbits; existing nonzero bootbits are
//! never overwritten unless they carry our versioned record signature.
//! This records boot identity, NOT a ZFS consistency seal or an image signature.
use super::{Session, Status};
use crate::boot::BootPlan;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::FileExt},
    path::Path,
};

const PAGE: usize = 4096;
const BOOTBITS: usize = 1024;
const MAGIC: &[u8; 16] = b"ZBM-RESUME-v1\0\0\0";
const SWAP: &[u8; 10] = b"SWAPSPACE2";
const IMAGE: &[u8; 10] = b"S1SUSPEND\0";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    session: Session,
    kernel: String,
    initrd: Option<String>,
    cmdline: Vec<String>,
}

fn error(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
fn hash_file(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(hex(&hash.finalize()))
}
pub fn valid_partuuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

pub fn verify_running_cmdline(expected: &[String]) -> io::Result<()> {
    let actual = shlex::split(fs::read_to_string("/proc/cmdline")?.trim())
        .ok_or_else(|| error("Invalid running kernel command line"))?;
    if actual != expected {
        return Err(error(
            "Running kernel arguments differ from the saved boot plan",
        ));
    }
    Ok(())
}

pub struct Swap {
    file: File,
}
impl Swap {
    pub fn open(partuuid: &str, write: bool) -> io::Result<Self> {
        Self::open_device(
            &super::device::Device::Partition {
                partuuid: partuuid.into(),
            },
            write,
        )
    }

    pub fn open_device(device: &super::device::Device, write: bool) -> io::Result<Self> {
        if unsafe { libc::sysconf(libc::_SC_PAGESIZE) } != PAGE as libc::c_long {
            return Err(error("Resume currently requires 4096-byte kernel pages"));
        }
        let file = device.open(write)?;
        // O_EXCL prevents cancellation of kernel-active swap; flock serializes
        // cooperating zbm-rs processes. Neither replaces durability barriers.
        if unsafe {
            libc::flock(
                file.as_raw_fd(),
                if write { libc::LOCK_EX } else { libc::LOCK_SH } | libc::LOCK_NB,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file })
    }
    fn header(&self) -> io::Result<[u8; PAGE]> {
        let mut h = [0; PAGE];
        self.file.read_exact_at(&mut h, 0)?;
        Ok(h)
    }
    pub fn status(&self) -> io::Result<Status> {
        status(&self.header()?)
    }

    /// Call while swap is inactive, before swapon and the hibernation request.
    /// The kernel preserves bootbits when marking the image. Never retrofit a
    /// record onto an already-created image.
    pub fn record(&self, pool_guid: u64, plan: &BootPlan) -> io::Result<()> {
        if pool_guid == 0 {
            return Err(error("Invalid pool GUID"));
        }
        let h = self.header()?;
        if &h[PAGE - 10..] != SWAP {
            return Err(error(
                "Resume recording requires inactive, formatted swap without an image",
            ));
        }
        if h[..BOOTBITS].iter().any(|b| *b != 0) && &h[..16] != MAGIC {
            return Err(error(
                "Swap bootbits are occupied; refusing to overwrite them",
            ));
        }
        if plan.target.snapshot.is_some() {
            return Err(error("Recording snapshot-clone sessions is not supported"));
        }
        let record = Record {
            session: Session {
                id: fs::read_to_string("/proc/sys/kernel/random/uuid")?
                    .trim()
                    .into(),
                pool_guid,
                target: plan.target.clone(),
            },
            kernel: hash_file(&plan.kernel)?,
            initrd: plan.initrd.as_deref().map(hash_file).transpose()?,
            cmdline: plan.cmdline.clone(),
        };
        let bytes = serde_json::to_vec(&record).map_err(|e| error(e.to_string()))?;
        let packed = zstd::stream::encode_all(bytes.as_slice(), 3)?;
        if packed.len() > BOOTBITS - 20 {
            return Err(error("Boot identity exceeds swap bootbits capacity"));
        }
        let mut bits = [0; BOOTBITS];
        bits[..16].copy_from_slice(MAGIC);
        bits[16..20].copy_from_slice(&(packed.len() as u32).to_le_bytes());
        bits[20..20 + packed.len()].copy_from_slice(&packed);
        self.file.write_all_at(&bits, 0)?;
        self.file.sync_all()?;
        if self.header()?[..BOOTBITS] != bits {
            return Err(error("Resume record verification failed"));
        }
        Ok(())
    }

    /// Restore only Linux's original swap signature. Payload is not erased.
    pub fn cancel(&self, expected: &str) -> io::Result<()> {
        let h = self.header()?;
        if &h[PAGE - 10..] != IMAGE || digest(&h) != expected {
            return Err(error("Saved image changed; cancellation consent expired"));
        }
        if &h[PAGE - 20..PAGE - 10] != SWAP {
            return Err(error("Unrecognized original swap signature"));
        }
        self.file.write_all_at(SWAP, (PAGE - 10) as u64)?;
        self.file.sync_all()?;
        let after = self.header()?;
        if &after[PAGE - 10..] != SWAP || after[..PAGE - 10] != h[..PAGE - 10] {
            return Err(error("Image cancellation could not be verified"));
        }
        Ok(())
    }

    pub fn verify_handoff(&self, pool_guid: u64, plan: &BootPlan) -> io::Result<()> {
        let h = self.header()?;
        let status = status(&h)?;
        if !status.matches(pool_guid, &plan.target) {
            return Err(error("Boot target does not match the saved session"));
        }
        let record = record(&h)?;
        if record.cmdline != plan.cmdline
            || record.kernel != hash_file(&plan.kernel)?
            || record.initrd != plan.initrd.as_deref().map(hash_file).transpose()?
        {
            return Err(error("Saved-session boot artifacts or arguments changed"));
        }
        Ok(())
    }

    /// Target-initramfs gate. The normal kernel resume mechanism performs its
    /// own image checks. A returned resume attempt is a failure, never a signal
    /// to mount ZFS and continue booting silently.
    pub fn restore(&self) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        if !Path::new("/etc/initrd-release").exists() {
            return Err(error("Resume restore is restricted to initramfs"));
        }
        let h = self.header()?;
        let pending = !matches!(status(&h)?, Status::Inactive);
        if fs::read_to_string("/proc/swaps")?
            .lines()
            .skip(1)
            .any(|l| !l.trim().is_empty())
        {
            return Err(error("Swap was activated before resume"));
        }
        if pending {
            let saved = record(&h)?;
            verify_running_cmdline(&saved.cmdline)?;
        }
        for line in fs::read_to_string("/proc/self/mountinfo")?.lines() {
            let kind = line
                .split(" - ")
                .nth(1)
                .and_then(|s| s.split_whitespace().next())
                .unwrap_or("");
            if ![
                "rootfs",
                "tmpfs",
                "ramfs",
                "devtmpfs",
                "proc",
                "sysfs",
                "devpts",
                "cgroup2",
                "securityfs",
                "efivarfs",
                "debugfs",
                "tracefs",
                "configfs",
                "pstore",
                "mqueue",
            ]
            .contains(&kind)
            {
                return Err(error(format!(
                    "Filesystem {kind} was mounted before resume"
                )));
            }
        }
        // Pool directories are present in kstat for imported pools, independently
        // of whether any dataset was mounted. Global ZFS kstats are regular files.
        match fs::read_dir("/proc/spl/kstat/zfs") {
            Ok(entries) => {
                for entry in entries {
                    if entry?.file_type()?.is_dir() {
                        return Err(error("A ZFS pool was imported before resume"));
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        let dev = self.file.metadata()?.rdev();
        fs::write("/sys/power/resume_offset", b"0")?;
        fs::write(
            "/sys/power/resume",
            format!("{}:{}", libc::major(dev), libc::minor(dev)),
        )?;
        if !pending {
            // Publish the dedicated device for systemd's next hibernation.
            // The verified normal swap signature contains no image to restore.
            if fs::read_to_string("/sys/power/resume")?.trim()
                != format!("{}:{}", libc::major(dev), libc::minor(dev))
            {
                return Err(error("Kernel did not accept the dedicated resume device"));
            }
            return Ok(());
        }
        Err(error(
            "Kernel resume returned without restoring the saved session",
        ))
    }
}

fn record(h: &[u8; PAGE]) -> io::Result<Record> {
    if &h[..16] != MAGIC {
        return Err(error("Image has no zbm-rs boot identity"));
    }
    let len = u32::from_le_bytes(h[16..20].try_into().unwrap()) as usize;
    if len == 0 || len > BOOTBITS - 20 {
        return Err(error("Invalid resume record size"));
    }
    let decoder = zstd::stream::read::Decoder::new(&h[20..20 + len])?;
    let mut bytes = Vec::new();
    decoder.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(error("Resume record exceeds decoded limit"));
    }
    serde_json::from_slice(&bytes).map_err(|e| error(e.to_string()))
}
fn status(h: &[u8; PAGE]) -> io::Result<Status> {
    if &h[PAGE - 10..] == SWAP {
        return Ok(Status::Inactive);
    }
    if &h[PAGE - 10..] != IMAGE {
        return Err(error(
            "Unknown swap/image format; refusing to assume no saved session",
        ));
    }
    if &h[PAGE - 20..PAGE - 10] != SWAP {
        return Err(error("Image has an unknown original swap signature"));
    }
    let id = digest(h);
    match record(h) {
        Ok(mut record) => {
            record.session.id = id;
            let status = Status::Pending {
                session: Box::new(record.session),
            };
            match status.validate() {
                Ok(()) => Ok(status),
                Err(reason) => Ok(Status::Unavailable {
                    session_id: digest(h),
                    reason: reason.into(),
                }),
            }
        }
        Err(e) => Ok(Status::Unavailable {
            session_id: id,
            reason: e.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        directory: std::path::PathBuf,
        swap: Swap,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = std::env::temp_dir().join(format!(
                "zbm-resume-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&directory).unwrap();
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(directory.join("swap"))
                .unwrap();
            file.set_len((PAGE * 2) as u64).unwrap();
            file.write_all_at(SWAP, (PAGE - 10) as u64).unwrap();
            Self {
                directory,
                swap: Swap { file },
            }
        }
        fn mark_image(&self) {
            self.swap
                .file
                .write_all_at(SWAP, (PAGE - 20) as u64)
                .unwrap();
            self.swap
                .file
                .write_all_at(IMAGE, (PAGE - 10) as u64)
                .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn cancellation_checks_identity_and_preserves_all_other_bytes() {
        let fixture = Fixture::new();
        fixture
            .swap
            .file
            .write_all_at(b"payload", PAGE as u64)
            .unwrap();
        fixture.mark_image();
        let before = fixture.swap.header().unwrap();
        assert!(fixture.swap.cancel("stale-consent").is_err());
        assert_eq!(before, fixture.swap.header().unwrap());
        fixture.swap.cancel(&digest(&before)).unwrap();
        assert!(matches!(fixture.swap.status().unwrap(), Status::Inactive));
        assert_eq!(
            &before[..PAGE - 10],
            &fixture.swap.header().unwrap()[..PAGE - 10]
        );
        let mut payload = [0; 7];
        fixture
            .swap
            .file
            .read_exact_at(&mut payload, PAGE as u64)
            .unwrap();
        assert_eq!(&payload, b"payload");
        assert!(fixture.swap.cancel(&digest(&before)).is_err());
    }

    #[test]
    fn native_record_roundtrip_pins_artifacts_and_refuses_foreign_bootbits() {
        let fixture = Fixture::new();
        let session = crate::resume::tests::fixture();
        let kernel = fixture.directory.join("kernel");
        fs::write(&kernel, b"kernel A").unwrap();
        let plan = BootPlan {
            target: session.target,
            kernel: kernel.clone(),
            initrd: None,
            cmdline: vec!["root=ZFS=tank/root".into()],
        };
        fixture.swap.record(42, &plan).unwrap();
        fixture.mark_image();
        let status = fixture.swap.status().unwrap();
        assert!(status.matches(42, &plan.target));
        fixture.swap.verify_handoff(42, &plan).unwrap();
        fs::write(&kernel, b"kernel B").unwrap();
        assert!(fixture.swap.verify_handoff(42, &plan).is_err());
        assert!(fixture.swap.record(42, &plan).is_err());
        fixture.swap.cancel(status.session_id().unwrap()).unwrap();
        fixture.swap.file.write_all_at(b"foreign", 0).unwrap();
        assert!(fixture.swap.record(42, &plan).is_err());
    }
    #[test]
    fn missing_identity_is_cancellable_but_never_resumable() {
        let mut h = [0; PAGE];
        h[PAGE - 10..].copy_from_slice(IMAGE);
        h[PAGE - 20..PAGE - 10].copy_from_slice(SWAP);
        assert!(matches!(status(&h).unwrap(), Status::Unavailable { .. }));
        h[..16].copy_from_slice(MAGIC);
        h[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(status(&h).unwrap(), Status::Unavailable { .. }));
        h[16..20].copy_from_slice(&4_u32.to_le_bytes());
        h[20..24].copy_from_slice(b"oops");
        assert!(matches!(status(&h).unwrap(), Status::Unavailable { .. }));
        h[PAGE - 10..].copy_from_slice(SWAP);
        assert!(matches!(status(&h).unwrap(), Status::Inactive));
        h[PAGE - 1] = 0;
        assert!(status(&h).is_err());
    }
}
