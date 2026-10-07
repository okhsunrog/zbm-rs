//! Owner authorization of exact boot inputs. CMS/X.509 is certificate-pinned:
//! embedded signer certificates and implicit CA trust are never accepted.
use crate::boot::BootPlan;
use openssl::{
    cms::{CMSOptions, CmsContentInfo},
    stack::Stack,
    x509::X509,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::File,
    io::{self, Read, Write},
    os::fd::FromRawFd,
    path::Path,
};

pub const AUTHORIZATION_LIMIT: usize = 1024 * 1024;
pub const ARTIFACT_LIMIT: u64 = 1024 * 1024 * 1024;

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub size: u64,
    pub sha256: String,
}
impl Artifact {
    pub fn read(path: &Path) -> io::Result<Self> {
        let mut input = File::open(path)?;
        let size = input.metadata()?.len();
        if !input.metadata()?.is_file() || size == 0 || size > ARTIFACT_LIMIT {
            return Err(invalid("Invalid boot artifact size or file type"));
        }
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut read = 0;
        loop {
            let count = input.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            read += count as u64;
            if read > ARTIFACT_LIMIT {
                return Err(invalid("Boot artifact grew beyond limit"));
            }
            hash.update(&buffer[..count]);
        }
        if read != size {
            return Err(invalid("Boot artifact changed while hashing"));
        }
        Ok(Self {
            size,
            sha256: hex(&hash.finalize()),
        })
    }
    fn validate(&self) -> io::Result<()> {
        if self.size == 0
            || self.size > ARTIFACT_LIMIT
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(invalid("Invalid signed artifact digest/size"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootRule {
    pub source_dataset: String,
    pub argument_index: usize,
    pub prefix: String,
    pub allow_snapshot_clones: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootAuthorization {
    pub version: u32,
    pub architecture: String,
    pub kernel: Artifact,
    pub initramfs: Artifact,
    /// Exact ordered tokens after removing the single typed root slot.
    pub fixed_arguments: Vec<String>,
    pub root: RootRule,
    /// Detached Linux security.ima signature; signed together with the inputs.
    pub initramfs_ima_signature: Vec<u8>,
}

impl BootAuthorization {
    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1 || self.architecture != std::env::consts::ARCH {
            return Err(invalid("Unsupported authorization version or architecture"));
        }
        self.kernel.validate()?;
        self.initramfs.validate()?;
        zfskit::names::DatasetName::parse(&self.root.source_dataset).map_err(invalid)?;
        if !matches!(
            self.root.prefix.as_str(),
            "root=ZFS=" | "root=zfs:" | "root="
        ) || (self.root.prefix == "root="
            && !self
                .fixed_arguments
                .iter()
                .any(|arg| arg == "rootfstype=zfs"))
            || self.root.argument_index > self.fixed_arguments.len()
            || self.fixed_arguments.len() > 64
            || self
                .fixed_arguments
                .iter()
                .any(|s| s.len() > 4096 || s.contains('\0') || s.starts_with("root="))
            || !(9..=8192).contains(&self.initramfs_ima_signature.len())
        {
            return Err(invalid("Invalid signed argument/root/IMA signature policy"));
        }
        Ok(())
    }
    pub fn artifact_id(&self) -> String {
        hex(&Sha256::digest(
            format!("{}:{}", self.kernel.sha256, self.initramfs.sha256).as_bytes(),
        ))
    }
    pub fn matches_plan(&self, plan: &BootPlan, owned_snapshot_clone: bool) -> io::Result<()> {
        self.matches_selection(plan)?;
        if plan.target.snapshot.is_some() && !owned_snapshot_clone {
            return Err(invalid(
                "Snapshot handoff requires broker-owned clone evidence",
            ));
        }
        Ok(())
    }
    /// Non-mutating authorization before a clone is created. This does not
    /// authorize handoff: matches_plan additionally requires actual ownership.
    pub fn matches_selection(&self, plan: &BootPlan) -> io::Result<()> {
        self.validate()?;
        let mut arguments = plan.cmdline.clone();
        let root = arguments
            .get(self.root.argument_index)
            .ok_or_else(|| invalid("Missing root argument"))?;
        if root != &format!("{}{}", self.root.prefix, plan.target.dataset) {
            return Err(invalid("Final root argument differs from selected dataset"));
        }
        arguments.remove(self.root.argument_index);
        if arguments != self.fixed_arguments {
            return Err(invalid("Final kernel arguments are not authorized"));
        }
        match &plan.target.snapshot {
            None if plan.target.dataset == self.root.source_dataset => {}
            Some(snapshot)
                if self.root.allow_snapshot_clones
                    && snapshot
                        .name
                        .split_once('@')
                        .is_some_and(|(source, _)| source == self.root.source_dataset) => {}
            _ => {
                return Err(invalid(
                    "Root selection or snapshot clone is not authorized",
                ));
            }
        }
        Ok(())
    }
}

pub struct Authorization {
    policy: BootAuthorization,
    payload_sha256: String,
    authority_sha256: String,
}
impl Authorization {
    pub fn load_for_plan(plan: &BootPlan, authority_paths: &[String]) -> io::Result<Self> {
        let kernel = Artifact::read(&plan.kernel)?;
        let initramfs = Artifact::read(
            plan.initrd
                .as_ref()
                .ok_or_else(|| invalid("Initramfs required"))?,
        )?;
        let id = hex(&Sha256::digest(
            format!("{}:{}", kernel.sha256, initramfs.sha256).as_bytes(),
        ));
        let path = crate::boot::rooted_path(
            &plan.target.root,
            &std::path::PathBuf::from(format!("/boot/zbm-rs/authorizations/{id}.json")),
        )?;
        let payload = read_bounded(&path, AUTHORIZATION_LIMIT)?;
        let signature = read_bounded(&path.with_extension("cms"), AUTHORIZATION_LIMIT)?;
        let authorities = authority_paths
            .iter()
            .map(|path| {
                let bytes = read_bounded(Path::new(path), 64 * 1024)?;
                X509::from_pem(&bytes)
                    .or_else(|_| X509::from_der(&bytes))
                    .map_err(invalid)
            })
            .collect::<io::Result<Vec<_>>>()?;
        let authorization = Self::verify(&payload, &signature, &authorities)?;
        if authorization.policy.kernel != kernel || authorization.policy.initramfs != initramfs {
            return Err(invalid("Signed artifacts differ from selected input bytes"));
        }
        Ok(authorization)
    }
    pub fn verify(payload: &[u8], signature: &[u8], authorities: &[X509]) -> io::Result<Self> {
        if payload.is_empty()
            || payload.len() > AUTHORIZATION_LIMIT
            || signature.is_empty()
            || signature.len() > AUTHORIZATION_LIMIT
        {
            return Err(invalid("Authorization or CMS signature exceeds bounds"));
        }
        // Parse directly into a deny_unknown_fields struct: duplicate JSON fields
        // are rejected, rather than normalized through a lossy JSON Value.
        let policy: BootAuthorization = serde_json::from_slice(payload).map_err(invalid)?;
        policy.validate()?;
        let mut last_error = "no configured authorities".to_owned();
        for authority in authorities {
            let mut cms = CmsContentInfo::from_der(signature).map_err(invalid)?;
            if cms.to_der().map_err(invalid)? != signature {
                return Err(invalid("CMS must be canonical DER without trailing bytes"));
            }
            let mut pinned = Stack::new().map_err(invalid)?;
            pinned.push(authority.clone()).map_err(invalid)?;
            let mut verified = Vec::new();
            let verified_signature = cms.verify(
                Some(&pinned),
                None,
                Some(payload),
                Some(&mut verified),
                CMSOptions::BINARY
                    | CMSOptions::DETACHED
                    | CMSOptions::NOINTERN
                    | CMSOptions::NOVERIFY,
            );
            if verified_signature.is_ok() && verified == payload {
                return Ok(Self {
                    policy,
                    payload_sha256: hex(&Sha256::digest(payload)),
                    authority_sha256: hex(&Sha256::digest(authority.to_der().map_err(invalid)?)),
                });
            }
            last_error = match verified_signature {
                Err(error) => error.to_string(),
                Ok(()) => "CMS verified content differs from detached payload".into(),
            };
        }
        Err(invalid(format!(
            "Boot authorization is not signed by a pinned owner authority: {last_error}"
        )))
    }
    pub fn policy(&self) -> &BootAuthorization {
        &self.policy
    }
    pub fn prepare(
        self,
        plan: &BootPlan,
        owned_snapshot_clone: bool,
    ) -> io::Result<VerifiedBootPlan> {
        self.policy.matches_plan(plan, owned_snapshot_clone)?;
        let kernel = PreparedInput::copy(&plan.kernel, &self.policy.kernel, None)?;
        let initramfs = PreparedInput::copy(
            plan.initrd
                .as_ref()
                .ok_or_else(|| invalid("Enforced ZFS boot requires initramfs"))?,
            &self.policy.initramfs,
            Some(&self.policy.initramfs_ima_signature),
        )?;
        Ok(VerifiedBootPlan {
            kernel,
            initramfs,
            arguments: plan.cmdline.clone(),
            evidence: VerifiedEvidence {
                authorization_sha256: self.payload_sha256,
                authority_sha256: self.authority_sha256,
                kernel_sha256: self.policy.kernel.sha256,
                initramfs_sha256: self.policy.initramfs.sha256,
                dataset: plan.target.dataset.clone(),
            },
        })
    }
}

#[derive(Debug, Serialize)]
pub struct VerifiedEvidence {
    pub authorization_sha256: String,
    pub authority_sha256: String,
    pub kernel_sha256: String,
    pub initramfs_sha256: String,
    pub dataset: String,
}

/// No unchecked constructor, Deserialize or writable/path-based artifact handles.
pub struct VerifiedBootPlan {
    kernel: PreparedInput,
    initramfs: PreparedInput,
    arguments: Vec<String>,
    evidence: VerifiedEvidence,
}
impl VerifiedBootPlan {
    pub fn kernel(&self) -> &File {
        &self.kernel.0
    }
    pub fn initramfs(&self) -> &File {
        &self.initramfs.0
    }
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }
    pub fn evidence(&self) -> &VerifiedEvidence {
        &self.evidence
    }
}

struct PreparedInput(File);
impl PreparedInput {
    fn copy(path: &Path, expected: &Artifact, ima: Option<&[u8]>) -> io::Result<Self> {
        let mut source = File::open(path)?;
        if !source.metadata()?.is_file() || source.metadata()?.len() != expected.size {
            return Err(invalid(
                "Boot input type or size differs from authorization",
            ));
        }
        let name = CString::new("zbm-rs-verified-input").unwrap();
        let fd = unsafe {
            libc::memfd_create(name.as_ptr(), libc::MFD_ALLOW_SEALING | libc::MFD_CLOEXEC)
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut staged = unsafe { File::from_raw_fd(fd) };
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut size = 0;
        loop {
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            size += count as u64;
            if size > expected.size {
                return Err(invalid("Boot input grew while preparing"));
            }
            hash.update(&buffer[..count]);
            staged.write_all(&buffer[..count])?;
        }
        if size != expected.size || hex(&hash.finalize()) != expected.sha256 {
            return Err(invalid("Boot input digest differs from authorization"));
        }
        if let Some(signature) = ima {
            let name = CString::new("security.ima").unwrap();
            if unsafe {
                libc::fsetxattr(
                    fd,
                    name.as_ptr(),
                    signature.as_ptr().cast(),
                    signature.len(),
                    0,
                )
            } < 0
            {
                return Err(io::Error::last_os_error());
            }
        }
        let seals =
            libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL;
        if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } < 0 {
            return Err(io::Error::last_os_error());
        }
        // kernel_read_file must not see an outstanding writer. Reopen read-only
        // and close the writer; the seals prevent later reopening for mutation.
        let readonly = File::open(format!("/proc/self/fd/{fd}"))?;
        drop(staged);
        Ok(Self(readonly))
    }
}

pub fn read_bounded(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("Expected a regular authorization file"));
    }
    let mut data = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut data)?;
    if data.len() > limit {
        return Err(invalid("Authorization input exceeds bounds"));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use openssl::{
        asn1::Asn1Time, hash::MessageDigest, pkey::PKey, rsa::Rsa, x509::X509NameBuilder,
    };
    use std::os::fd::AsRawFd;
    fn signer() -> (X509, PKey<openssl::pkey::Private>) {
        let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "Disposable test authority")
            .unwrap();
        let name = name.build();
        let mut cert = X509::builder().unwrap();
        cert.set_version(2).unwrap();
        let serial = openssl::bn::BigNum::from_u32(1)
            .unwrap()
            .to_asn1_integer()
            .unwrap();
        cert.set_serial_number(&serial).unwrap();
        cert.set_subject_name(&name).unwrap();
        cert.set_issuer_name(&name).unwrap();
        cert.set_pubkey(&key).unwrap();
        cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
            .unwrap();
        cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
            .unwrap();
        cert.sign(&key, MessageDigest::sha256()).unwrap();
        (cert.build(), key)
    }
    fn policy() -> BootAuthorization {
        BootAuthorization {
            version: 1,
            architecture: std::env::consts::ARCH.into(),
            kernel: Artifact {
                size: 1,
                sha256: "11".repeat(32),
            },
            initramfs: Artifact {
                size: 1,
                sha256: "22".repeat(32),
            },
            fixed_arguments: vec!["rw".into()],
            root: RootRule {
                source_dataset: "tank/root".into(),
                prefix: "root=ZFS=".into(),
                argument_index: 0,
                allow_snapshot_clones: true,
            },
            initramfs_ima_signature: vec![3; 64],
        }
    }
    #[test]
    fn cms_requires_exact_payload_and_pinned_signer() {
        let (cert, key) = signer();
        let (other, _) = signer();
        let payload = serde_json::to_vec(&policy()).unwrap();
        let sig = CmsContentInfo::sign(
            Some(&cert),
            Some(&key),
            None,
            Some(&payload),
            CMSOptions::BINARY | CMSOptions::DETACHED,
        )
        .unwrap()
        .to_der()
        .unwrap();
        let accepted = Authorization::verify(&payload, &sig, std::slice::from_ref(&cert));
        assert!(accepted.is_ok(), "{:?}", accepted.err());
        assert!(Authorization::verify(&payload, &sig, &[other]).is_err());
        let mut altered = payload.clone();
        altered.push(b' ');
        assert!(Authorization::verify(&altered, &sig, std::slice::from_ref(&cert)).is_err());
        let mut altered = sig.clone();
        altered.push(0);
        assert!(Authorization::verify(&payload, &altered, std::slice::from_ref(&cert)).is_err());
    }
    #[test]
    fn unsupported_and_ambiguous_policies_are_rejected() {
        let mut p = policy();
        p.version = 2;
        assert!(p.validate().is_err());
        p.version = 1;
        p.fixed_arguments.push("root=ZFS=tank/other".into());
        assert!(p.validate().is_err());
        let payload = br#"{"version":1,"version":1}"#;
        assert!(serde_json::from_slice::<BootAuthorization>(payload).is_err());
    }
    #[test]
    fn authorization_binds_arguments_source_and_owned_snapshot_handoff() {
        use crate::boot::{Backend, BootInputs, BootTarget, SnapshotSource};
        let p = policy();
        let target = BootTarget {
            dataset: "tank/root".into(),
            generation: None,
            label: "fixture".into(),
            root: "/fixture".into(),
            toplevel: None,
            snapshot: None,
            mountpoint: Some("/".into()),
            issue: None,
            backend: Backend::Linux {
                rootprefix: "root=ZFS=".into(),
                commandline: vec![],
            },
            inputs: BootInputs {
                kernel: "/boot/kernel".into(),
                initrd: Some("/boot/initrd".into()),
                init: None,
                kernel_params: vec![],
            },
        };
        let mut plan = BootPlan {
            target,
            kernel: "/fixture/kernel".into(),
            initrd: Some("/fixture/initrd".into()),
            cmdline: vec!["root=ZFS=tank/root".into(), "rw".into()],
        };
        assert!(p.matches_plan(&plan, false).is_ok());
        plan.cmdline.push("init=/bin/sh".into());
        assert!(p.matches_plan(&plan, false).is_err());
        plan.cmdline.pop();
        plan.target.dataset = "tank/other".into();
        plan.cmdline[0] = "root=ZFS=tank/other".into();
        assert!(p.matches_plan(&plan, true).is_err());
        plan.target.snapshot = Some(SnapshotSource {
            name: "tank/root@approved".into(),
            guid: 42,
        });
        assert!(p.matches_selection(&plan).is_ok());
        assert!(p.matches_plan(&plan, false).is_err());
        assert!(p.matches_plan(&plan, true).is_ok());
        plan.target.snapshot.as_mut().unwrap().name = "tank/foreign@approved".into();
        assert!(p.matches_plan(&plan, true).is_err());
    }
    #[test]
    fn prepared_inode_is_sealed_and_readonly() {
        let path = std::env::temp_dir().join(format!("zbm-seal-test-{}", std::process::id()));
        std::fs::write(&path, b"original").unwrap();
        let digest = Artifact::read(&path).unwrap();
        let prepared = PreparedInput::copy(&path, &digest, None).unwrap();
        std::fs::write(&path, b"modified").unwrap();
        let mut reader = prepared.0.try_clone().unwrap();
        let mut data = Vec::new();
        reader.read_to_end(&mut data).unwrap();
        assert_eq!(data, b"original");
        let seals = unsafe { libc::fcntl(prepared.0.as_raw_fd(), libc::F_GET_SEALS) };
        assert_eq!(
            seals,
            libc::F_SEAL_WRITE | libc::F_SEAL_GROW | libc::F_SEAL_SHRINK | libc::F_SEAL_SEAL
        );
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .open(format!("/proc/self/fd/{}", prepared.0.as_raw_fd()))
            .unwrap();
        assert!(writer.write_all(b"bad").is_err());
        std::fs::remove_file(path).unwrap();
    }
}
