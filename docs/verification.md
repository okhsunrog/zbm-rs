# Verification and supported scope

Boot acceptance requires the selected OS to start with the expected root and
system closure. Menu visibility, a successful build and a prepared kexec image
are separate observations.

## Coverage

| Area | Recorded validation |
| --- | --- |
| Rust and immutable configuration | Unit/integration checks, formatting, Clippy, Cargo input tracking and Nix/Rust configuration parity. |
| Startup and direct boot | Automatic catalog, startup passphrase with cancellation/retry, resumed kernel selection, single-Enter encrypted Arch boot, countdown cancellation and immediate Arch boot in disposable VMs. |
| Local key dependencies | An encrypted non-BE keysource unlocks a dependent BE, resumes its requested kernel chooser and leaves no temporary key file or passphrase in serial diagnostics. |
| Nix images | Kernel/ZFS matching, staged ELF dependencies, public trust inputs and size limits. |
| Generic Linux | Arch ZFS-root boot and snapshot, rollback, clone and promotion scenarios. |
| Encrypted Arch | Hidden passphrase input, cancellation/wrong-key refusal, ordinary and writable snapshot-clone boot; also through enforced OVMF Secure Boot, CMS/IMA and `zfs=` root authorization. |
| Framework image integration | DMI-scoped recovery module built for Linux 7.2.9, immutable module options and required early loading checked in staged images. The owner also booted the deployed loader and NixOS on a physical Framework Laptop 13; see the physical results below. |
| NixOS | Exact-generation ZFS-root boot and writable snapshot-clone boot, preserving the source dataset and snapshot. |
| NixOS specialisations | Menu selection checks the exact child closure; native installation prepares signed parent/child inputs, mounts distinct home datasets, and restores prepared links after a missing child secret. This does not establish enforced Secure Boot for the variant. |
| Protected loader | OVMF Secure Boot; forced kernel/module signatures, lockdown, signed IMA policy, sealed boot inputs and an unprivileged UI. |
| Target authorization | Kernel/initramfs/argument rejection, pinned CMS policy, authorized root selection and actual signed second-kernel boot. |
| TPM | PCR11/PCR15 replay, read-only startup, no loader SRK/NV allocation, required absence refusal and optional absence with target checks preserved. |
| Native OS TPM setup | The NixOS fixture creates its own SRK and initializes native v261 NvPCRs without masks or loader-policy conflicts. |
| Experimental authenticated hibernation | Linux 7.2.9 through patch 0028 restored RAM and created the next image with CRB/SHA-256 and TIS/dual-bank TPMs, LUKS2 swap and ZFS. Signature-trailer/payload tampering and TPM clear were rejected before pool import; consumed attempts remained protected on the next boot. The complete OVMF Secure Boot/PE/IMA/CMS enforced loader-to-target chain restored RAM and created the next image. |
| TPM interruption and repeat cycles | CRB/SHA-256 restored the same RAM session three times. Unavailable TPM stopped resume before pool import without consuming the image; restoring TPM and the clean initramfs allowed retry and another complete cycle, retaining the original boot ID, RAM data and target kexec prohibition. |
| Compressed authenticated images | CRB/SHA-256 passed three restorations with default compression and TPM-absence/retry. The separate full Secure Boot/PE/IMA/CMS chain passed all seven checks with compression too. Actual compression/decompression messages were asserted, rather than inferred from configuration. |
| Suspend-then-hibernate | Encrypted ZFS/LUKS2 fixture passed preparation refusal, byte-preserving activation refusal for pending images, early wake, failed-hibernation suspend fallback and two successive timed RAM restorations. The Linux 7.2.9 CRB/SHA-256 variant with runtime systemd 262.1 and compression also passed TPM absence/refusal/retry and a subsequent direct cycle (four authenticated restorations total). These use real s2idle with injected ACPI power-button wakes. |
| Empty-swap repair without TPM | Explicit repair boot with `systemd.tpm2_wait=0` passed: healthy ZFS and ordinary paging, no hibernation, integrity Lockdown retained. This parameter does not bypass the native image gate. |
| Cancellation-only rescue | Verified cancellation/UI scenarios and FAT candidate/known-good publication passed. The exact production EFI artifact passed OVMF boot, menu/F2 and F10. On the owner's Framework, firmware rejected unsigned rescue with Secure Boot enabled; with it disabled, rescue booted and loaded NixOS. |
| Recovery and lifecycle | Manager faults, console restoration, process reaping, restart, protected recovery and clean poweroff. |

The table records source/VM coverage and the specific physical observations below.
Target v262 signed NvPCR integration and across-kexec PCR15 log transport remain
open gates.
The ordinary TPM rows describe the userspace measurement policy. The opt-in
hibernation kernel separately provisions and protects its RFC-specific NV index;
its physical validation applies to the explicit experimental deployment below. See
[hibernation architecture and validation](hibernation-design.md).

## Physical Framework validation (2026-11-01)

The owner's Framework Laptop 13 (Intel Core Ultra Series 1) runs NixOS with
encrypted ZFS, dedicated LUKS2 swap and the patched Linux 7.2.9 kernel.

- The signed enforced zbm-rs image booted with firmware Secure Boot enabled and
  loaded the selected NixOS specialisation.
- Firmware rejected the unsigned rescue with Secure Boot enabled. After the owner
  disabled Secure Boot, rescue booted and loaded the Niri specialisation.
- Repeated direct hibernation/resume cycles succeeded, requested through
  `systemctl hibernate` and the desktop shell. The kernel reported
  `Restored authenticated NULL-primary context` and
  `Hibernation image restored successfully`; ZFS reported healthy pools afterward.

These results describe the installed Framework deployment. The VM negative tests
and the broader acceptance matrix below retain their separately stated scope.

## Reproduction and records

Follow [development testing](development.md) for ordinary images and
[the protected-image workflow](security-testing.md) for signed OVMF/TPM scenarios.
The [October 2026 records](history/verification-2026-10.md) preserve concrete run
identities, failures and earlier profiles.

## Secure Boot acceptance matrix

This is the full matrix for [the accepted design](secure-boot-model.md). The
coverage table above records exercised boundaries; this matrix also includes
remaining negative, deployment and physical gates. Ordinary unsigned boot/lifecycle
tests alone cannot establish the signed chain, IMA appraisal or protected recovery.

Use fresh disposable OVMF variable stores with enrolled **fixture** keys and actual
Secure Boot enforcement. Build through the canonical image pipeline, then sign
output copies in an isolated fixture deployment step. Never use host disks, host
firmware variables, host signing/encryption keys or host configuration. Fixture
private keys must stay outside production image outputs; production has no test
SSH or fault hooks. Preserve artifact digests, public certificate fingerprints,
firmware state, startup enforcement evidence, serial/UI logs and target boot IDs.

| Boundary | Required scenarios / outcome |
| --- | --- |
| Firmware -> loader UKI | Trusted signed UKI boots; unsigned, changed and untrusted/revoked loader signatures are refused by firmware, not merely by the loader UI. |
| Image configuration / startup | Missing/invalid policy, missing trust roots, inactive required enforcement and insecure loader arguments refuse handoff without falling back to development defaults. |
| Firmware-state policy | `enforce` with optional firmware requirement continues target checks when Secure Boot is disabled; requiring firmware refuses disabled/unknown state. Unknown state never downgrades. |
| BootAuthorization | Valid Linux/NixOS targets boot; bad signature, unknown authority, out-of-scope authority, malformed/duplicate fields, wrong architecture/version and altered digests fail. |
| Target kernel | Unsigned, altered or untrusted signed kernel fails in `kexec_file_load`, including when a fixture authorization signs its digest. Accepted kernel identity matches the selected target handshake. |
| Target initramfs | Altered, unsigned, missing required or incorrectly signed initramfs fails appraisal. Independently exercise broker digest rejection and kernel IMA rejection with an otherwise authorized digest. |
| Arguments | Approved exact tokens and permitted root slot succeed; extra/repeated/conflicting arguments, changed `init=`, unauthorized root and unapproved `kernelArgs` fail. |
| Immutable input preparation | Symlink/path swaps and source/prepared-inode mutation races cannot change the bytes consumed after verification. Cover detached IMA signatures on private staged copies and snapshots without xattrs. |
| Snapshots / clones | Authorized snapshot/owned-clone boot reports the correct root and boot ID; unsigned historical targets, mismatched origins, ownership reuse and unauthorized selection fail without changing read-only snapshots. |
| Module trust / bypasses | Unsigned ZFS/other preboot modules fail; trusted modules load. Direct legacy kexec and unauthorized manager/broker requests cannot bypass policy; a signed kernel alone cannot authorize arbitrary initramfs/arguments. |
| Failure / recovery | Manager panic/abort, PID-1 failure/last-ditch recovery, failed appraisal and returned kexec before/after unlock offer no unrestricted shell. Diagnostics/restart/reboot/poweroff remain usable. |
| Lifecycle / cleanup | Cancelled or failed preparation unloads the owned kernel and cleans owned mounts/imports/clones only; IPC closure, overlapping requests and manager restart do not retain stale authorization. |
| Deployment / rotation | Deployed bytes match signed outputs. Exercise certificate overlap, unknown/revoked authorities and a trusted recovery target without assuming a new loader revokes older signed loader images. |
| TPM ownership / readiness | Read-only TPM2/PCR15 probe; required absence refuses startup, optional absence preserves target checks; loader creates no SRK/NV indices and no target masks are needed. |
| Prepared-target measurement | PCR11 replays from stub measurements without loader phases; exact verified plan/arguments replay in PCR15; rejected inputs and manager recovery do not advance PCR11 or allocate TPM objects. |
| OS TPM / log transport | OS creates/reuses its own SRK and initializes its own NvPCRs. Native v261 compatibility passes; target v262 signed-policy authorization and across-kexec PCR15 event/plan transport need separate acceptance. |
| UI evidence | Firmware state, policy, authorization, kernel acceptance and initramfs appraisal are distinct. No claim that the selected root's contents are attested. |

Positive boot acceptance must reach the selected OS, not stop at a loader “PASS”
message. Exercise both Linux and NixOS targets and authorized snapshot/clone root
variation through real kexec. Negative tests must identify which independent
layer rejected the input; broker rejection alone does not prove kernel appraisal.

Physical acceptance is a separate gate on the intended hardware, firmware trust
configuration and deployed image: verify enrolled keys, trusted boot and recovery,
loader ZFS/module operation, target module/DKMS trust and authorized snapshot boot.
Record precisely what was observed after reboot. VM acceptance does not establish
Framework firmware behavior or recovery accessibility on a physical console.
Target-wide IMA, TPM unlock/attestation and strong antirollback remain outside this
initial gate. The deferred untrusted-boot UI action has no current acceptance claim.
