# Native hibernation integration

This implementation is experimental and disabled by default. It changes neither
installed images nor host sleep settings. It does not implement ZFS hibernation
cookies or prove that a pool was unchanged while the machine was off. The
experimental kernel backend authenticates RAM separately. Authenticated RAM
restoration, subsequent hibernation and the complete protected chain passed in
disposable VMs. Secure Boot loading and repeated authenticated hibernation/resume
also passed on the owner's Framework Laptop 13 (Intel Core Ultra Series 1), with
encrypted ZFS and LUKS2 swap; see the [physical results](verification.md#physical-framework-validation-2026-11-01).
It must not be advertised as making ZFS hibernation safe.

## Existing flow

Keep the pools, environments, generations/kernels and snapshot views. There is no
new startup question or automatic action. A recognized session selects
its exact environment/target by default. Menu/countdown/immediate retain their
existing timing. Enter on that target resumes; the footer says Resume. Unlock
uses the existing password panel. Cancelling the countdown or navigating does
not cancel the saved session.

While an image exists, F2 additionally offers **Discard saved session (stay in
menu)** and **Cold boot selected system (discard saved session)**. Both use the
existing confirmation and verified image cancellation. Standalone discard has
no boot continuation. Cold boot pins the selected target before cancellation,
including the saved generation when selecting a whole environment whose current
default differs. The broker rejects cold boot while any image remains; handoff
rechecks that a newly appearing image cannot turn cold boot into resume. No new
bare keyboard shortcuts are assigned, and these actions are hidden without an
image. An unreadable or still-locked swap cannot be treated as a cancellable
known image.

The context line shows Resume pending, the target carries a RESUME tag, and
Selection/details explain the action. Compact layouts retain the context line.
Errors and operation progress keep their normal priority.

Navigation, search, help, details, snapshot inspection, rescan, manager restart
and power off preserve the session. F10 works from dialogs, including unlock and
cancellation confirmation, without another question. Letter P retains its
context-sensitive behavior; typing a password or search query never powers off.

Another target, generation or specialisation; snapshot-clone boot; clone
preparation/deletion/creation; promotion; rollback; and an unrestricted shell
require confirmed cancellation. Warn when executing, never when highlighting or
inspecting. Esc preserves the session and selection. Existing discard/promote/
rollback dialogs include the session-loss warning without a second question.
Rollback retains typed ROLLBACK. Security-disabled actions do not cancel anything.
Q/crash recovery cannot expose an unguarded shell while a session is pending.

## Internal ownership

`zbm_core::resume` models the session. `resume::linux::Swap` implements bounded
Linux header parsing, boot-record storage, artifact checks and image cancellation.
The TUI coordinator calls it directly in ordinary mode and through the existing
privileged broker in enforced mode; there is no external service or socket
protocol. PID 1 uses the same status for recovery. Target initramfs uses the same
binary and module through `--resume-restore`.

Runtime imports are read-only at pool level while a saved session exists,
including broker rediscovery. An already writable import is refused. Configured
read-only policy remains independent and is never silently upgraded. After
confirmed cancellation, a temporarily read-only owned import can be reopened for
an authorized mutation. A status read error is never treated as no session.

## Linux on-disk format

Support is limited to dedicated plain or LUKS2 GPT swap partitions, 4096-byte
kernel pages and native Linux S1SUSPEND. Zvols, swapfiles and alternate page/image
formats are unsupported. Existing plain images may retain `resume.swap_partuuid`;
new images use `resume.device`.

`resume::device::Device` pins the outer PARTUUID, LUKS UUID and mapping name,
checks the actual backing partition and requires a single dm-crypt target.
The loader journals mapping ownership in root-only RAM before activation.
Foreign mappings are never adopted or closed. Integrity and reencryption layouts
are refused before activation. A suspended or mismatched mapping is an error,
never an empty session. No key material is read from `dmsetup table`.

`nix build .#checks.x86_64-linux.luks2-resume-device` exercises this API with real
GPT/LUKS2 devices inside a disposable VM. The fixture uses synthetic kernel
signatures to test detection and cancellation; it does not claim to restore
hibernated RAM. It covers closed containers, a wrong key, duplicate LUKS UUIDs
on different partitions, foreign/forged and mixed-target mappings, read-only
mappings, suspension, active swap, changed boot artifacts, expired cancellation
consent and malformed headers.

Linux reserves the first 1024 bytes of swap for bootbits and preserves them when
marking a hibernation image. Before swapon/hibernation, zbm-rs writes a versioned,
size-bounded compressed boot record there: a nonce, pool/target identity, hashes
of kernel/initramfs, and exact planned arguments. Existing nonzero foreign
bootbits are refused. A record is never added to an existing image. The record
is boot routing metadata, not a ZFS cookie or an authenticated memory image.

Image detection uses the actual kernel signature. A missing/invalid boot record
leaves a cancellable image with Resume unavailable; it never guesses a target.
Consent is bound to a hash of the current header. Cancellation reopens the block
device exclusively (rejecting active swap), rechecks the header, restores only
the original SWAPSPACE2 signature, syncs, and reads back the result. Interrupted
attempt metadata, including damaged owned records, is cleared under the same
explicit cancellation consent. It preserves
the payload; this is invalidation, not secure erasure. Unknown signatures and
changed images are refused.

An in-progress marker in root-owned /run prevents a failed or interrupted write
from becoming apparently successful via page-cache reads after manager restart.
Power off remains available. A subsequent boot inspects the on-disk signature
again; ambiguous/partial headers remain blocked. Successful cancellation removes
the marker only after flush and verification. The marker itself never authorizes
resume or replaces image invalidation.

Boot-record protocol v2 tracks consumed images; legacy v1 images without a boot
ID are cancellable but cannot resume. A normal inactive v1 record can be replaced
when preparing the next hibernation.

Linux consumes S1SUSPEND before completing RAM-image verification. The target
therefore first persists an `attempting` flag in the existing swap boot record,
with flush and readback, then drops its exclusive writer before invoking the
kernel resume interface. If Linux consumes the image and verification fails or
power is lost, SWAPSPACE2 plus this flag remains a protected, cancellable session.
A damaged owned record also requires explicit cancellation. Neither is treated
as an empty swap on the following boot. A valid image not yet consumed can still
be retried. Images predating this protocol remain cancellable but cannot resume.

The post-hibernation hook temporarily deactivates swap and calls
`--resume-finish-config`. Only the original kernel boot ID saved before sleep
can clear the flag after a successful RAM restore; a fresh kernel cannot do so.
Failure to finish keeps the next boot conservative. The hook reactivates swap
even on failure. This is recovery bookkeeping, not cryptographic evidence or a
pool consistency seal. `--resume-status-config` inspects this state without
modifying the device. Unit tests cover consumed images, stale consent, a wrong
boot ID and torn records. The ordinary Linux 6.18.55 encrypted-root VM restored
RAM, retained its boot ID, completed the finish hook, and matched direct and
buffered header reads. The TPM-authenticated CRB/SHA-256 and TIS/dual-bank VMs
restored RAM and successfully created the next signed image. Signature-trailer
and actual RAM-payload tampering, plus a cleared guest TPM, were rejected before
pool import; consumed images remained protected on the next boot. The separate
complete Secure Boot/PE/IMA/CMS loader-to-target run also restored RAM, retained
the early kexec prohibition and created the next image. These are VM results,
not physical Framework acceptance.

If the kernel cannot hibernate, a verified empty swap may still cold-boot after
the same filesystem/pool preflight. This supports repair through rescue without
TPM provenance. No image or incomplete attempt can use that branch, and it does
not initialize a hibernation device. The encrypted-root VM passed this path with
`nohibernate`, including rejection of an early read-only pool import, healthy
ZFS and ordinary paging after cold boot.
The TPM-unavailable repair variant supplies `systemd.tpm2_wait=0`: its deliberate
driver unbind leaves firmware advertising a device that systemd would otherwise
wait for after switch-root. This changes systemd's synchronization only; the
native resume gate still rejects missing TPM when an image exists.

The authenticated CRB/SHA-256 test restored the same session three times,
preserving its original boot ID and RAM-only file. Between restorations it
unbound the guest TPM: resume stopped before pool import without consuming the
image. Restoring the exact original initramfs and TPM allowed retry, another
hibernation and a third restoration. The finish hook returned the boot record
to inactive and the target kexec prohibition remained set.
The same three-cycle test passed with the kernel's default image compression.
The complete protected-chain run separately passed a compressed RAM restore,
the next hibernation and both explicit cancellation/cold-boot UI paths. Tests
asserted actual compression and decompression, not only the planned arguments.

## Encrypted swap and credential ownership

The first credential source is explicitly selected `zfs-unlock`: reuse the
passphrase for an image-pinned pool GUID and encryption root. `zfs-keyfile` and
other sources are not implemented and are rejected, never used as fallbacks.
This is not a requirement that every ZFS root share its password with LUKS.
The credential is the user's passphrase, not either filesystem's internal key.
Independent ZFS and LUKS volume keys remain independent. Passphrase and key-file
handling must preserve each backend's byte semantics, including newline handling.
Unlocking ZFS is not proof that those same bytes can unlock LUKS.

The existing password panel supplies both consumers through the privileged unlock
module. It loads the ZFS key without mounting a dataset, opens LUKS using secret
stdin, then drops the secret. An already unlocked root still verifies the supplied
credential if its swap is closed. No secret is retained across menu actions or
passed on the kernel command line.

`Locked` is distinct from `Blocked`: it permits metadata-only read-only catalogs
and unlocking exactly the configured root, without boot-root mounts, swapon or an
unrestricted shell. A correct ZFS password which fails LUKS preserves this state.
Inspection, not merely successful unlocking, determines whether an image exists.

The owned crypt mapping permits the confirmed cancellation writer; inspection
and handoff open read-only file descriptors. Cancellation requires an exclusive
device open and fresh consent. This avoids retaining the password to reopen a
read-only mapping later. Final image verification precedes closing the owned
mapping for kexec. The target opens its own mapping using its prepared initramfs
secret; the loader never appends a newly entered secret to an initramfs.

If the loader needs ZFS metadata or boot files before handoff, every relevant
import must be read-only at pool level from its first import. `-N` or a read-only
dataset mount alone is insufficient. Key loading and inspection must not upgrade
that policy. Unmount and export all loader-owned pools before handoff. The target
initramfs retains the stricter existing gate: unlock the configured swap and
attempt resume before any ZFS pool import or persistent filesystem mount.

Credential reuse does not prove that a pool was unchanged during hibernation and
does not authenticate the saved RAM image for kernel Lockdown. It removes an
unnecessary root/key-file dependency; it does not replace either safety boundary.

## NixOS integration

The optional `boot.loader.zbm-rs.hibernation` module integration adds a native
pre-hibernation record step and a target-initramfs resume gate. Its resolved
non-secret `resumeConfig` is the policy for both roles. `programs.zbm-rs` uses it
by default; custom `lib.mkImage` users pass it as `loaderConfig.resume`. The entire
policy must match, not just the PARTUUID. Legacy plain configurations retain
`swapPartuuid`. The swap partition must
already be configured/active; the module neither formats it nor partitions disks.
The record step temporarily swaps it off, records the booted NixOS target, and
reactivates it when verified inactive, including after ordinary recording errors.
Reactivation checks the same device identity and resume state, then uses the
kernel's `swapon` syscall on the pinned file descriptor. It never uses util-linux's
automatic image-signature rewriting. A pending image, damaged record or unfinished
attempt leaves swap off and reports failure. Failure aborts hibernation. Memory pressure
can make swapoff fail; that is not overridden. Both `systemd-hibernate.service`
and `systemd-suspend-then-hibernate.service` use fail-closed `ExecStartPre`
preparation and `ExecStopPost` completion. The composite service does not start
the direct hibernation service. Its record covers the whole operation: initial
suspend, optional battery-estimation wakes, hibernation and systemd's suspend
fallback if hibernation fails. Preparation alone does not mark an image pending;
only the real Linux image signature does. Early manual wake leaves ordinary
inactive swap, while completion refuses to clear a still-present image or an
attempt belonging to another boot. System-sleep scripts cannot provide this
gate because systemd ignores their errors.

Suspend-then-hibernate remains off by default. Opt in using native NixOS settings;
ordinary Suspend is unchanged and hybrid-sleep remains disabled:

```nix
systemd.sleep.settings.Sleep = {
  AllowSuspendThenHibernate = true;
  HibernateDelaySec = "2h";
};
```

Request it with `systemctl suspend-then-hibernate`. Systemd owns wake alarms,
battery policy and the transition; no extra timer or daemon is installed. Test
the physical machine's alarm wake separately from VM acceptance. The disposable
Q35 composite fixture uses real s2idle and real hibernation, but injects an ACPI
power-button wake because CMOS alarms did not reliably wake that guest. A wake
before the long deadline tests ordinary return; a wake after the short deadline
tests systemd's timer-expiry path. This does not establish automatic RTC delivery
or battery-alarm delivery on a laptop. Persistent
initramfs mounts must all use the gated ZFS imports;
configurations with other early persistent filesystems are rejected.

```nix
boot.loader.zbm-rs.hibernation = {
  enable = true;
  swap = {
    type = "luks2";
    partuuid = "12345678-1234-1234-1234-123456789abc";
    luksDevice = "nvme-swap"; # boot.initrd.luks.devices.nvme-swap
  };
  credential = {
    type = "zfs-unlock";
    poolGuid = "13099373940756918353";
    encryptionRoot = "pool/nixos/root";
  };
};
```

The named LUKS device must use `/dev/disk/by-uuid/<LUKS-UUID>` and a `keyFile`
provided by `boot.initrd.secrets`. The module derives its identity and mapping,
includes the native binary/configuration and crypt tools in the initramfs, and
requires `copy-secrets -> cryptsetup -> zbm-resume -> zfs-import`. Only paths and
identities enter Nix configuration, never key contents. Existing target runtime
secret delivery remains independent of the loader prompt.

`hibernate=noresume` disables the kernel's automatic attempt; the stock systemd
resume generator and units are suppressed in the target initramfs. Plain
`noresume` is unsuitable because systemd also treats it as disabling hibernation
device selection. Every configured ZFS import requires
successful completion of zbm-resume.service, rather than merely being ordered
after it. The native gate verifies the saved arguments and refuses to run after
persistent filesystems are mounted or ZFS pools are imported. It invokes the
kernel resume interface explicitly. A returned resume attempt fails the gate;
it never silently falls back to mounting ZFS.

On a cold boot with an ordinary swap signature, the gate initializes the kernel
resume device for the next systemd hibernation. Recording verifies the currently
mounted root dataset and actual running arguments against the published plan.

The operator must explicitly opt into NixOS's unsafeAllowHibernation policy.
Enforced resume requires the authenticated backend and signed capability
described below. Its disposable VM acceptance and the recorded Framework
deployment checks have passed. ESP diagnostics must be
disabled until their writes can be coordinated with saved filesystem state.
These restrictions are not waived by the UI.

## Agreed protected-boot architecture

The existing guard above is implemented in ordinary and enforced modes. The
experimental Linux 7.2.9 TPM-signature patch set passed authenticated RAM restore,
subsequent hibernation and the complete protected-loader chain in disposable VMs.
The following trust chain and negative tests define that supported path. Host
enablement still requires a matching owner-signed build and physical acceptance.

### Kernel roles and authenticated memory

The signed enforced loader, target OS and unsigned rescue have separate runtime
roles. Rescue retains its independent source/package set. The loader permits
outgoing `kexec_file_load` only after owner authorization of the exact target.

The agreed Framework baseline shares the protected `kernelPackages` between
loader and target, retaining kexec in both. Separate kernel builds are not a
requirement. The current host trusts full root with locally readable boot/IMA
signing keys; compromise of those keys already permits signing arbitrary
kernel/module inputs. This is a threat-model boundary, not a proof of TPM
provenance across arbitrary later kexec calls.

Once the complete path works and passes acceptance, remove the older system
generations using the normal generation/pruning workflow. Do not remove them
before the working replacement has been verified. Deliberately chaining an old
kernel or researching its possible TPM-provenance bypass is outside this
deployment's implementation and acceptance scope.

The agreed target-only hardening is the kernel command-line parameter
`sysctl.kernel.kexec_load_disabled=1`. Linux applies it before executing
initramfs `/init`, which starts with further loads already prohibited, without
a custom initramfs service. The Framework NixOS configuration adds it to
target Bootspec arguments; the separately constructed loader and rescue command
lines omit it and retain outgoing kexec. The kernel builds remain shared. This
policy does not interfere with receiving the loader handoff or restoring RAM;
the restored target must retain the prohibition too. The parameter belongs in
the exact signed target arguments.

The sysctl rejects further `kexec_load`/`kexec_file_load` calls, including crash
image loads, and cannot be reset through sysctl during that kernel boot. It does
not unload an already loaded image, which is why setting it before `/init`
matters. Compile-time exclusion of target kexec is not required; reconsider it
only if the threat model or key isolation changes. A full root compromise with
usable signing keys remains outside this boundary.

Matthew Garrett's TPM-signature design is the selected experimental backend.
Both protected kernels must preserve kernel ownership of its reserved TPM NV
state and the EFI/PCR5 provenance boundary. Only the target needs the full
hibernation writer and verifier. The loader does not need to create or restore
RAM images, but keeping that support in the shared kernel is acceptable under the
agreed baseline. The current helper uses one full experimental configuration;
no separate loader/target builds are required solely by their runtime roles.

Keep integrity Lockdown and signature enforcement. LUKS2 encrypts swap; TPM
signatures authenticate the saved RAM image. This is not support for
confidentiality Lockdown. Never replace image authentication with a Lockdown
exception or interpret a successful LUKS unlock as authenticated memory.

The first successful restore exposed a repeat-cycle defect: restored RAM held
the previous TPM boot's cached NULL-primary context. The experimental backend
`linux-tpm-signature-v2` adds a one-shot context handoff through reserved EFI ACPI
memory excluded from the RAM image. The signed `LNXHPV02` image header binds
the original NULL name and the physical handoff address. Full image verification
and an exact address match must precede handoff preparation; matching E820 ranges
alone does not bind an allocation's offset within an EFI pool.

Copying a fresh context alone would introduce an unauthenticated trust step.
The verifier therefore challenges the same AK that authenticated the saved
image's signing-key provenance with a fresh random nonce. Its signed session
audit must match the locally computed transcript for the fixed-template NULL
CreatePrimary operation. Only then is the current NULL context exported. After
RAM restoration, TPM resume imports it only when the old cached NULL identity
matches the signed original name, consumes it once, and wipes the handoff.
New kexec initialization and abandoned restoration clear it too. In-session
reset detection stays enabled; stale hardware handles are not flushed against
the new TPM context. Runtime checks passed on disposable CRB and TIS TPMs. This
is a local experimental extension, not an upstream-approved change or a solution
to all physical TPM bus attacks and cold-boot trust assumptions.

Verify EFI/PCR5 evidence and TPM NV protection across the actual supported
patched-loader-to-patched-target handoff before enabling this backend. Do not
assume PCR5 is reset by kexec or waive image-tampering and invalid/unavailable-TPM
tests. The main loader's profile restriction covers its own handoffs, not
subsequent syscalls from the running OS. This deployment does not claim an
end-to-end enforced chain against arbitrary privileged kexec transitions.

### Owner authorization and compatibility

BootAuthorization version 2 adds a `protected_hibernation` profile. Its version
is 1, backend is `linux-tpm-signature-v2`, and `record_protocol` is 2. The immutable
build contract additionally requires context-handoff protocol 1; legacy kernel
metadata cannot advertise this backend. The signed
`resume_policy_sha256` binds the dedicated partition/LUKS UUIDs, mapping name and
credential source (pool GUID and encryption root). UUID case, decimal GUID
spelling and legacy plain-partition syntax normalize; mapping/dataset names
remain case sensitive. Shared Rust/Python vectors check the policy identity.
The surrounding authorization still binds exact kernel/initramfs bytes,
ordered arguments and the typed root selection. Version 1 authorizations remain
readable and never gain this capability implicitly.

The NixOS installation hook supplies an immutable build contract from its own
configuration, never a discovered `prepared.json`. The contract checks the
effective protected-kernel configuration and rendered resume/import units. The
publisher compares actual kernel payload bytes with the approved shared build
(allowing only PE signing metadata), checks that the final initrd retains the
approved base byte-for-byte, and permits only declared regular secrets in an
appended archive. It rejects code/policy replacements and verifies the final
PE/IMA/CMS signatures. A kernel version string, ordinary PE signature, runtime
marker or manual allowlist cannot substitute for these checks. This trusts the
owner's Nix build/install hook; it is not remote build attestation.

The broker checks this profile before clone creation and again before every
verified handoff when resume is configured, including cold boots. Missing,
unsupported or mismatched profiles fail, as does a missing/conflicting early
target kexec prohibition. The publisher and broker checks passed native TPM
and complete protected-chain VM testing, including cold-boot refusal for missing
capabilities or a different resume policy. After successful migration,
old generations are removed rather than retained as a required recovery path;
independent rescue remains available to repair the system. The exact schema and
artifact validator follow the supported-path provenance tests rather than
claiming capabilities the backport has not demonstrated.

### Resume and cancellation ordering

The normal menu/countdown/immediate flow and exact saved target remain the
default. Read-only loader imports, credential acquisition, boot-file inspection,
verified cancellation and the stricter target resume-before-import gate remain
separate boundaries. The target never silently falls back to a writable cold
boot after a failed restore. ESP diagnostics remain incompatible until their
writes are coordinated too. Any future pool-key-file credential source must
preserve these boundaries; it is not implemented today.

Rescue is an independently booted, unsigned EFI image requiring firmware Secure
Boot to be disabled. `resume.role = "cancel-only"` converts recognized saved
images to cancellable, unavailable sessions; the handoff and configured
record/restore utilities reject restoration. Its image command line includes
`nohibernate`, disabling the kernel hibernation interface before userspace.
It does not advertise
the protected TPM-hibernation capability. It retains outgoing kexec for recovery.
It must still inspect the configured swap before the first pool import and use
the same cancellation-only guard. Entering rescue, navigating or powering off
does not erase an image. Booting any OS or opening an unrestricted shell first
requires confirmed, synced and verified image invalidation. Rescue never resumes.
The cancellation-only role passed the disposable UI scenario: direct configured
restore is refused, Enter requires cancellation confirmation, Esc preserves the
image, and standalone discard stays in the menu. The ordinary flow and F10's
whole-disk preservation check also passed. Integration into the independent
candidate/known-good deployment is implemented and passed the disposable FAT
publication test. The host configuration source pins this guarded baseline;
its exact production rescue artifact also passed firmware boot, menu, F2 and
poweroff checks in a disposable VM. On the owner's Framework, firmware rejected
the unsigned rescue with Secure Boot enabled; with it disabled, rescue booted
and loaded the selected NixOS specialisation. Saved-session cancellation itself
retains the VM coverage described above. An older pinned
image without the guard must not be presented as satisfying this contract.

See [independent rescue publication](nixos-deployment.md#optional-efi-publication-and-independent-rescue)
for the independent installer and candidate/known-good lifecycle.

## Remaining safety boundary and validation

Header identity and artifact hashes cannot detect arbitrary intervening writes
to ZFS. A strong consistency seal needs kernel/ZFS cooperation tied to the actual
hibernation snapshot; a pre-sleep TXG reading is not sufficient. Independent
bootloaders/rescue systems can bypass loader restrictions. Raw swap stores RAM
contents without encryption; LUKS2 protects confidentiality but does not
authenticate the RAM image or satisfy kernel Lockdown. These risks remain.

Tests cover wrong/stale image identity, cancellation preserving other bytes,
foreign bootbits, corrupt metadata, artifact changes, normal navigation, modal
Esc/F10, combined rollback confirmation, compact rendering and disabled defaults.
Source/unit checks are not physical resume acceptance. Before any installation,
exercise the exact image and target initramfs in disposable VMs, including power
loss, failed resume, restart, other generations and read-only import behavior;
then validate the actual laptop separately.

`cargo xtask resume-smoke --image IMAGE --run FRESH_DIRECTORY` exercises real
ZFS/LUKS and the actual password/cancellation UI with synthetic image headers:
different secrets, foreign mappings, restart, another generation sharing a kernel,
cancellation before recovery shell and power-off preserving the logical disk.
It also verifies both explicit F2 actions, stale-consent rejection without boot,
standalone cancellation staying in the same kernel/menu, and a real cold kexec
into the loader fixture with the pinned saved generation's arguments. The latter
is a handoff check, not a complete NixOS boot or RAM restoration.
The encrypted-hibernation VM additionally checks the target-only early kexec
prohibition, rejection of both load interfaces and attempts to reset the sysctl,
and preservation of the prohibition across actual RAM restoration. Bootstrap
still performs the incoming kexec with its own prohibition unset.
`nix build .#checks.x86_64-linux.encrypted-hibernation` instead installs a disposable
encrypted ZFS-root NixOS, rejects an intentionally early read-only pool import,
and exercises real RAM restoration through the target initramfs. It verifies the
unchanged kernel boot ID, a surviving RAM-only file and pool health after resume.
The intervening bootstrap uses separate disks and never imports the hibernated
pool or opens its swap. Neither is physical or enforced/Secure Boot acceptance.

The optional `lib.mkTpmHibernationProbe` and `lib.mkTpmHibernationTest` factories
accept the same experimental `kernelPackages` plus public signed kernel/module
artifacts. The first checks EFI/PCR5 and the reserved NV index across
`kexec_file_load`; the second extends the encrypted-root fixture with actual
authenticated RAM restoration and signature-trailer or actual RAM-payload
corruption, TPM reset and unavailable-TPM retry. The
negative case requires signature rejection and failure of the resume gate before
the pool-import predecessor can run. This fixture uses uncompressed images to
isolate signature validation from decompression/CRC failures. Neither factory
enables firmware Secure Boot or the loader's enforced BootAuthorization/IMA
handoff, so those remain separate tests. Signing keys never enter the guest or
Nix derivations. The complete chain has its own runtime-only
`lib.mkProtectedHibernationDriver`; see [the reproduction workflow](security-testing.md#authenticated-hibernation-chain)
and the patch-set README for recorded VM results, and
[verification](verification.md) for the physical deployment observations.
