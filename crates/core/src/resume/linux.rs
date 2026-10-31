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
pub const RECORD_PROTOCOL: u32 = 2;
const MAGIC: &[u8; 16] = b"ZBM-RESUME-v2\0\0\0";
const LEGACY_MAGIC: &[u8; 16] = b"ZBM-RESUME-v1\0\0\0";
const SWAP: &[u8; 10] = b"SWAPSPACE2";
const IMAGE: &[u8; 10] = b"S1SUSPEND\0";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    session: Session,
    kernel: String,
    initrd: Option<String>,
    cmdline: Vec<String>,
    #[serde(default)]
    boot_id: String,
    #[serde(default, skip_serializing_if = "is_false")]
    attempting: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}
fn owned_record(h: &[u8; PAGE]) -> bool {
    &h[..16] == MAGIC || &h[..16] == LEGACY_MAGIC
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

    /// Reactivate verified empty swap without util-linux's automatic rewriting
    /// of hibernation signatures. Open read-only so the kernel can claim it.
    pub fn activate(&self) -> io::Result<()> {
        if !matches!(self.status()?, Status::Inactive) {
            return Err(error(
                "Cannot activate swap while a saved session or unfinished attempt remains",
            ));
        }
        // Pin the validated device across activation, rather than resolving a
        // potentially replaced /dev link again. The kernel independently checks
        // its swap signature; a newly appearing image fails without rewriting.
        let path = std::ffi::CString::new(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
            .expect("numeric file descriptor contains no NUL");
        if unsafe { libc::swapon(path.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Call while swap is inactive, before swapon and the hibernation request.
    /// The kernel preserves bootbits when marking the image. Never retrofit a
    /// record onto an already-created image.
    pub fn record(&self, pool_guid: u64, plan: &BootPlan) -> io::Result<()> {
        if pool_guid == 0 {
            return Err(error("Invalid pool GUID"));
        }
        let h = self.header()?;
        if &h[PAGE - 10..] != SWAP || !matches!(status(&h)?, Status::Inactive) {
            return Err(error(
                "Resume recording requires inactive, formatted swap without an image",
            ));
        }
        if h[..BOOTBITS].iter().any(|b| *b != 0) && !owned_record(&h) {
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
            boot_id: fs::read_to_string("/proc/sys/kernel/random/boot_id")?
                .trim()
                .into(),
            attempting: false,
        };
        self.write_record(&record, &h)
    }

    fn write_record(&self, record: &Record, before: &[u8; PAGE]) -> io::Result<()> {
        let bytes = serde_json::to_vec(record).map_err(|e| error(e.to_string()))?;
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
        let after = self.header()?;
        if after[..BOOTBITS] != bits || after[BOOTBITS..] != before[BOOTBITS..] {
            return Err(error("Resume record verification failed"));
        }
        Ok(())
    }

    /// Persist before the kernel consumes S1SUSPEND. A failed or interrupted
    /// verifier must not turn the following boot into an apparently cold boot.
    /// Drop this exclusive writer before invoking the kernel resume interface.
    pub fn arm_attempt(&self, expected: &str) -> io::Result<()> {
        let h = self.header()?;
        if digest(&h) != expected || &h[PAGE - 10..] != IMAGE {
            return Err(error("Saved image changed before arming resume"));
        }
        let mut record = record(&h)?;
        if !valid_partuuid(&record.boot_id) {
            return Err(error(
                "Image predates durable resume tracking; discard the saved session",
            ));
        }
        record.attempting = true;
        self.write_record(&record, &h)
    }

    /// Only the restored kernel has the boot ID saved before hibernation.
    /// Run after systemd-hibernate returns, with swap temporarily deactivated.
    pub fn finish_attempt(&self) -> io::Result<()> {
        let h = self.header()?;
        if &h[PAGE - 10..] != SWAP {
            return Err(error("Cannot finish a resume while an image remains"));
        }
        if !owned_record(&h) {
            return Ok(());
        }
        let mut record = record(&h)?;
        if !record.attempting {
            return Ok(());
        }
        if record.boot_id != fs::read_to_string("/proc/sys/kernel/random/boot_id")?.trim() {
            return Err(error(
                "Only the restored kernel may finish the resume attempt",
            ));
        }
        record.attempting = false;
        self.write_record(&record, &h)
    }

    /// Restore only Linux's original swap signature. Payload is not erased.
    pub fn cancel(&self, expected: &str) -> io::Result<()> {
        let mut h = self.header()?;
        let record = record(&h).ok();
        let attempted = record.as_ref().is_some_and(|record| record.attempting);
        if digest(&h) != expected
            || (&h[PAGE - 10..] != IMAGE
                && !(&h[PAGE - 10..] == SWAP
                    && owned_record(&h)
                    && (attempted || record.is_none())))
        {
            return Err(error("Saved image changed; cancellation consent expired"));
        }
        if &h[PAGE - 10..] == IMAGE && &h[PAGE - 20..PAGE - 10] != SWAP {
            return Err(error("Unrecognized original swap signature"));
        }
        if owned_record(&h) && (attempted || record.is_none()) {
            // Consent also discards an interrupted attempt's routing metadata.
            // Clear it before the Linux signature: interruption stays blocked.
            h[..BOOTBITS].fill(0);
            self.file.write_all_at(&h[..BOOTBITS], 0)?;
            self.file.sync_all()?;
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
            if &h[PAGE - 10..] != IMAGE || !saved.attempting {
                return Err(error(
                    "Previous resume did not complete; explicit image cancellation is required",
                ));
            }
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
                "bpf",
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
        if !pending
            && !fs::read_to_string("/sys/power/state")?
                .split_whitespace()
                .any(|state| state == "disk")
        {
            // A rescue handoff may lack TPM provenance or explicitly disable
            // hibernation. Verified empty swap may cold-boot in that case;
            // pending images and incomplete attempts never take this branch.
            eprintln!("Hibernation unavailable; verified empty swap permits cold boot");
            return Ok(());
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
    if !owned_record(h) {
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
        if owned_record(h) {
            match record(h) {
                Ok(record) if !record.attempting => (),
                _ => return Ok(Status::Unavailable {
                    session_id: digest(h),
                    reason: "Previous resume attempt did not finish or its record is damaged; discard the saved session before booting or modifying pools".into(),
                }),
            }
        }
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
            if !valid_partuuid(&record.boot_id) {
                return Ok(Status::Unavailable {
                    session_id: id,
                    reason: "Image predates durable resume tracking; discard the saved session"
                        .into(),
                });
            }
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
        fn recorded_image(&self) {
            let kernel = self.directory.join("kernel");
            fs::write(&kernel, b"kernel").unwrap();
            self.swap
                .record(
                    42,
                    &BootPlan {
                        target: crate::resume::tests::fixture().target,
                        kernel,
                        initrd: None,
                        cmdline: vec!["root=ZFS=tank/root".into()],
                    },
                )
                .unwrap();
            self.mark_image();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    #[test]
    fn activation_refuses_images_and_consumed_or_damaged_attempts_without_writes() {
        let fixture = Fixture::new();
        fixture.recorded_image();
        let h = fixture.swap.header().unwrap();
        assert!(
            fixture
                .swap
                .activate()
                .unwrap_err()
                .to_string()
                .contains("saved session")
        );
        assert_eq!(h, fixture.swap.header().unwrap());
        fixture.swap.arm_attempt(&digest(&h)).unwrap();
        fixture
            .swap
            .file
            .write_all_at(SWAP, (PAGE - 10) as u64)
            .unwrap();
        let h = fixture.swap.header().unwrap();
        assert!(
            fixture
                .swap
                .activate()
                .unwrap_err()
                .to_string()
                .contains("saved session")
        );
        assert_eq!(h, fixture.swap.header().unwrap());
        fixture.swap.file.write_all_at(b"oops", 20).unwrap();
        let h = fixture.swap.header().unwrap();
        assert!(fixture.swap.activate().is_err());
        assert_eq!(h, fixture.swap.header().unwrap());
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
        assert!(matches!(status(&h).unwrap(), Status::Unavailable { .. }));
        h[PAGE - 1] = 0;
        assert!(status(&h).is_err());
    }

    #[test]
    fn consumed_image_stays_protected_until_confirmed_discard() {
        let fixture = Fixture::new();
        fixture.recorded_image();
        fixture
            .swap
            .file
            .write_all_at(b"payload", PAGE as u64)
            .unwrap();
        let before = fixture.swap.header().unwrap();
        fixture.swap.arm_attempt(&digest(&before)).unwrap();
        assert!(record(&fixture.swap.header().unwrap()).unwrap().attempting);
        assert!(fixture.swap.arm_attempt(&digest(&before)).is_err());
        // The kernel clears the Linux signature before authenticating RAM.
        fixture
            .swap
            .file
            .write_all_at(SWAP, (PAGE - 10) as u64)
            .unwrap();
        fixture.swap.file.sync_all().unwrap();
        let status = fixture.swap.status().unwrap();
        assert!(matches!(status, Status::Unavailable { .. }));
        fixture.swap.cancel(status.session_id().unwrap()).unwrap();
        assert!(matches!(fixture.swap.status().unwrap(), Status::Inactive));
        let mut payload = [0; 7];
        fixture
            .swap
            .file
            .read_exact_at(&mut payload, PAGE as u64)
            .unwrap();
        assert_eq!(&payload, b"payload");
    }

    #[test]
    fn only_original_boot_can_finish_and_torn_attempt_is_cancellable() {
        let fixture = Fixture::new();
        fixture.recorded_image();
        fixture
            .swap
            .arm_attempt(&digest(&fixture.swap.header().unwrap()))
            .unwrap();
        fixture
            .swap
            .file
            .write_all_at(SWAP, (PAGE - 10) as u64)
            .unwrap();
        let h = fixture.swap.header().unwrap();
        let mut record = record(&h).unwrap();
        let original_id = record.boot_id.clone();
        record.boot_id = "12345678-1234-1234-1234-123456789abc".into();
        fixture.swap.write_record(&record, &h).unwrap();
        assert!(fixture.swap.finish_attempt().is_err());
        assert!(fixture.swap.status().unwrap().protected());
        record.boot_id = original_id;
        fixture
            .swap
            .write_record(&record, &fixture.swap.header().unwrap())
            .unwrap();
        fixture.swap.finish_attempt().unwrap();
        assert!(matches!(fixture.swap.status().unwrap(), Status::Inactive));
        // A partially persisted record must not look like an empty session.
        fixture.swap.file.write_all_at(b"oops", 20).unwrap();
        let status = fixture.swap.status().unwrap();
        assert!(status.protected());
        fixture.swap.cancel(status.session_id().unwrap()).unwrap();
        assert!(matches!(fixture.swap.status().unwrap(), Status::Inactive));
    }

    #[test]
    fn legacy_image_needs_discard_and_next_record_upgrades_the_protocol() {
        let fixture = Fixture::new();
        fixture.recorded_image();
        let h = fixture.swap.header().unwrap();
        let mut old = record(&h).unwrap();
        old.boot_id.clear();
        fixture.swap.write_record(&old, &h).unwrap();
        fixture.swap.file.write_all_at(LEGACY_MAGIC, 0).unwrap();
        let status = fixture.swap.status().unwrap();
        assert!(matches!(status, Status::Unavailable { .. }));
        assert!(
            fixture
                .swap
                .arm_attempt(status.session_id().unwrap())
                .is_err()
        );
        fixture.swap.cancel(status.session_id().unwrap()).unwrap();
        fixture.recorded_image();
        assert_eq!(&fixture.swap.header().unwrap()[..16], MAGIC);
        assert!(matches!(
            fixture.swap.status().unwrap(),
            Status::Pending { .. }
        ));
    }
}
