# Native hibernation integration

This implementation is experimental and disabled by default. It changes neither
installed images nor host sleep settings. It does not implement ZFS hibernation
cookies, authenticate a saved RAM image, or prove that a pool was unchanged while
the machine was off. It must not be advertised as making ZFS hibernation safe.

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
the original SWAPSPACE2 signature, syncs, and reads back the result. It preserves
the payload; this is invalidation, not secure erasure. Unknown signatures and
changed images are refused.

An in-progress marker in root-owned /run prevents a failed or interrupted write
from becoming apparently successful via page-cache reads after manager restart.
Power off remains available. A subsequent boot inspects the on-disk signature
again; ambiguous/partial headers remain blocked. Successful cancellation removes
the marker only after flush and verification. The marker itself never authorizes
resume or replaces image invalidation.

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
reactivates it even if recording fails. Failure aborts hibernation. Memory pressure
can make swapoff fail; that is not overridden. This hooks direct hibernation;
suspend-then-hibernate and hybrid-sleep are disabled while this integration is
enabled. Persistent initramfs mounts must all use the gated ZFS imports;
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
The loader rejects this feature with enforced security because image
authentication/lockdown integration is not implemented. ESP diagnostics must be
disabled until their writes can be coordinated with saved filesystem state.
These restrictions are not waived by the UI.

## Agreed protected-boot architecture (not yet implemented)

The existing guard above is implemented in ordinary mode. The experimental
Linux 7.2.9 TPM-signature patch set is a build checkpoint, not a validated
protected-resume implementation. Enforced resume stays rejected until the
following trust chain and negative tests are complete.

### Kernel roles and authenticated memory

The signed enforced loader, target OS and unsigned rescue have separate runtime
roles. Rescue retains its independent source/package set. The loader permits
outgoing `kexec_file_load` only after owner authorization of the exact target.

The proposed Framework adjustment is to share the protected `kernelPackages`
between loader and target, retaining kexec in both. The earlier mandatory
compile-time split is under reconsideration: the current host trusts full root
with locally readable boot/IMA signing keys, and no concrete delegated
`CAP_SYS_BOOT`-without-signing-keys path has been identified in the reviewed
configuration. Compromise of those keys already permits signing arbitrary
kernel/module inputs. This is a threat-model boundary, not a fix for TPM
provenance across an arbitrary later kexec. Finalizing this adjustment does not
waive the provenance tests below.

Disabling target `CONFIG_KEXEC` and `CONFIG_KEXEC_FILE` remains optional stronger
hardening, particularly if signing keys move out of reach of the running OS or
keyless privileged attackers enter the threat model. It disables outgoing kexec,
not receipt of the loader handoff. An early target-only
`kernel.kexec_load_disabled=1` policy can instead share kernel builds, but must
precede untrusted userspace; it cannot prevent execution of an already loaded
image. Neither policy is enabled as part of this design discussion.

Matthew Garrett's TPM-signature design is the selected experimental backend.
Both protected kernels must preserve kernel ownership of its reserved TPM NV
state and the EFI/PCR5 provenance boundary. Only the target needs the full
hibernation writer and verifier. The loader does not need to create or restore
RAM images, but keeping that support in a shared kernel is acceptable under the
proposed baseline. The current helper uses one full experimental configuration;
no separate loader/target builds are required solely by their runtime roles.

Keep integrity Lockdown and signature enforcement. LUKS2 encrypts swap; TPM
signatures authenticate the saved RAM image. This is not support for
confidentiality Lockdown. Never replace image authentication with a Lockdown
exception or interpret a successful LUKS unlock as authenticated memory.

The unresolved EFI-to-kexec provenance must be tested before enabling this
backend. A positive patched-to-patched handoff alone is insufficient. A loader
which establishes TPM evidence must not transfer it to an old signed kernel
without the corresponding protection, even when no hibernation image currently
exists. Otherwise that kernel could invalidate the assumptions behind later
TPM evidence. Test unsupported targets and adversarial TPM operations as well
as the intended path; do not assume PCR5 is reset by kexec. The main loader's
profile restriction covers its own handoffs, not subsequent syscalls from the
running OS. If the shared-kernel baseline is adopted, record this residual risk
explicitly rather than describing it as an end-to-end enforced kexec chain.

### Owner authorization and compatibility

Extend the signed BootAuthorization with a versioned protected-hibernation
profile covering the exact kernel, initramfs, arguments and root selection.
The publisher must validate the required build artifacts/configuration before
signing this profile. A kernel version string, unsigned `prepared.json`, manual
allowlist or possession of an ordinary PE signature is insufficient. Existing
authorizations do not acquire the capability implicitly.

The capable loader checks this profile for every outgoing boot, including cold
boots, and rejects missing or unsupported profiles. Older generations remain
available through independent rescue, subject to image cancellation. The exact
schema and artifact validator follow the provenance experiment rather than
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
Boot to be disabled. Its kernel has hibernation disabled and does not advertise
the protected TPM-hibernation capability. It retains outgoing kexec for recovery.
It must still inspect the configured swap before the first pool import and use
the same cancellation-only guard. Entering rescue, navigating or powering off
does not erase an image. Booting any OS or opening an unrestricted shell first
requires confirmed, synced and verified image invalidation. Rescue never resumes.
This rescue role and its configuration are planned; an older pinned image
without the guard must not be presented as satisfying this contract.

See [independent rescue publication](nixos-deployment.md#optional-efi-publication-and-independent-rescue)
for the existing installer and the planned candidate/known-good lifecycle.

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
`nix build .#checks.x86_64-linux.encrypted-hibernation` instead installs a disposable
encrypted ZFS-root NixOS, rejects an intentionally early read-only pool import,
and exercises real RAM restoration through the target initramfs. It verifies the
unchanged kernel boot ID, a surviving RAM-only file and pool health after resume.
The intervening bootstrap uses separate disks and never imports the hibernated
pool or opens its swap. Neither is physical or enforced/Secure Boot acceptance.
