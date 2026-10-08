# Architecture

Boot environment discovery reads effective ZFS properties through zfskit after
an explicit selected-pool import. Visibility uses mountpoint and inherited
org.zfsbootmenu:active, with bootfs selecting the default environment. Only
mountable unencrypted candidates are mounted read-only under the ownership
ledger. Non-boot filesystems are left alone. The TUI separates pools, boot
environments, Linux kernel pairs and NixOS generations. Boot targets have an
explicit Linux or NixOS backend; generation, toplevel and init are optional for
Linux. ZBM kernel/commandline/rootprefix properties drive Linux selection and
root arguments; Bootspec remains authoritative for NixOS.
Mount failures are attached to the affected BE. Generation discovery preserves
usable targets and separate rejected-generation diagnostics, including when
browsing snapshots; one malformed Bootspec never invalidates its peers.

The manager owns one asynchronous boot operation at a time. It freezes selection
and further mutations while still handling Shell/Restart/Power off. OS-root
Bootspec reads and kernel loading use blocking workers outside the input loop.
Operations have a 30-second deadline; an expired operation requires a fresh
manager before further boot actions. Cancellation leaves ownership intent for
reconciliation, without upgrading imports or automatically destroying clones.
Runtime shutdown waits at most 100 ms for blocking workers before process exit;
PID 1 remains responsible for terminating/reaping the manager's process group.

The TUI keeps its view history, filters, list cursors and panels in transient
presentation state. Candidate rows carry typed identities; a rejected generation
never maps to a BootTarget even if a previously valid index remains in domain
telemetry. One command catalog drives keyboard mappings, help and the action menu;
one availability function gates dispatch. Search text is isolated from letter
shortcuts, and function-key recovery remains available. Modified key chords cannot
fall through to bare clone/power bindings or confirm clone discard.
Wide consoles render details beside the list; compact consoles expose a scrollable
details panel. Details render Bootspec inputs captured during discovery, without
filesystem reads or ZFS effects on selection. The executor re-resolves and validates
inputs before handoff. A successfully prepared plan can additionally show the real
clone root and rewritten command line. UI telemetry is separate from core state.

Snapshot targets retain source dataset, snapshot name and GUID. Browsing mounts
the immutable snapshot read-only and uses the same backend discovery. Clone
preparation, identity verification and optional discard live in core, separate
from the TUI and kernel executor. NixOS root override inspects `/init` in the actual initramfs and requires
systemd, without a custom Bootspec capability or runtime initrd construction.
Linux uses its native ZBM root prefix.
Prepared clone intent is journaled before ZFS creation; local ownership token,
origin and source GUID are checked after creation and before reuse. Handoff
marks the clone retained; conservative failures leave it intact. No automatic
deletion or recursive destruction occurs. Confirmed rollback is a separate
ZFS operation using `rollback -r` on exactly one selected dataset. Persistent
clone/promotion is likewise independent of boot discovery. Both check pool
ownership, writable policy and snapshot GUID; rollback validates all mounts
before unmounting owned inspections and never force-destroys dependent clones.
The UI requires exact `ROLLBACK` text; input cannot invoke bare letter shortcuts
inside confirmation. Mutations rediscover BEs, invalidating stale targets. An interrupted marker/journal
update may require manual recovery rather than risking reuse or data loss.

## One ELF, separate runtime roles

The image contains /bin/zbm-rs and /init -> /bin/zbm-rs. main checks PID 1 before
argument parsing, Tokio, Ratatui or ZFS initialization. PID 1 runs supervisor;
it spawns current_exe() as a fresh manager generation. Outside initramfs the
normal entry is manager; --manager --preview is explicit development mode.

The supervisor lifecycle uses std/libc and the small private protocol. It also
reads the image-owned typed JSON once through the shared config schema; it never
initializes Tokio, Ratatui or ZFS application state.
It mounts proc/sys/dev/run/tmp/devpts plus EFI/securityfs when available, runs the image's driver/udev bootstrap child,
saves tty1 state and supervises managers and emergency shells. The manager owns
ZFS initialization, Tokio, zfskit, pool discovery and the Ratatui event loop.
There is no zbm-init artifact and no shell/systemd/getty underneath PID 1.
In `enforce`, trusted IMA initialization precedes manager launch. The manager
starts the same executable as a root broker through a private bounded stream,
then drops all IDs/groups/capabilities before Tokio/UI startup. The broker stays
in the owned manager process group; PID 1 terminates and reaps the whole group.
Boot candidates received from the UI are re-resolved by the broker. UI-owned
diagnostics cannot overwrite readiness, trust, clone ownership or authorization
evidence. Diagnostic output and VM control descriptors are opened before the
privilege drop. Production has no VM control socket.

SIGCHLD, SIGTERM, SIGINT, SIGHUP and SIGQUIT have deliberate PID-1 handlers.
Handlers only set a lock-free bit mask. The synchronous loop forwards the four
control signals to the current child's process group and reaps all adopted
children. waitid(WNOWAIT) reserves the group leader's PID until group cleanup,
then waitpid reaps it. A manager crash cannot cause PID 1 to exit.

The parent restores saved termios, keyboard/display modes, active tty1 and ANSI
screen/cursor state after every generation. With the default JSON policy, a first failure restarts the manager;
a second unexpected termination within 30 seconds stops automatic restart and
opens an emergency shell. Exiting that shell explicitly starts a fresh manager.
Intentional Restart/Shell reset the failure budget. Unannounced exit, including
status 0, is unexpected; manager Q therefore does not terminate PID 1.

The supervisor's own unwind/error boundary retains the original console and
active child identity. Its last-ditch path kills/reaps the owned process group,
restores the console and starts an emergency shell. Exit retries supervision
without remounting /run or duplicating the completed bootstrap. If recovery itself
fails, PID 1 requests reboot; an unsuccessful reboot leaves a bounded-delay
recovery retry loop. Production has no supervisor fault injection hooks.

The shell behavior above applies only to `off`. The implemented enforced branch
replaces shell recovery, including last-ditch errors, with restart (only after
successful early security setup), reboot and poweroff. A failed setup does not
enter an automatic reboot loop. The signed OVMF synthetic-handoff scenario
checks these paths. Menu/broker NixOS ZFS-root and trusted snapshot-clone tests also
pass; generic-Linux and physical acceptance remain outstanding.

## Private lifecycle protocol

A private Unix datagram socketpair carries one typed byte: Restart, EmergencyShell,
Reboot, PowerOff or KexecStarting. Packets are bounded and validated. Both ends are
CLOEXEC by default; only the child's endpoint is cleared in pre_exec, and manager
entry immediately restores CLOEXEC before runtime creation or spawning tools.
PID 1 watches the actual process lifetime rather than waiting for socket EOF.
After intent, manager exit is bounded (2 seconds; 30 for a future kexec attempt).

Shutdown requests are acted upon only after clean manager termination. Successful
kexec leaves the old kernel; returning after KexecStarting is failed handoff and
enters recovery, not a crash loop. The lifecycle scenario tests failed handoff;
the separate NixOS boot scenario exercises actual kernel handoff and requires a
success marker from the selected OS.

## Initial boot backend

core owns BootTarget, BootPlan, guest-root-aware Bootspec path resolution and
explicit import/mount reconciliation. TUI selects a pool or generation; the
manager executor loads through kexec_file_load and executes the handoff.
Absolute OS symlinks resolve within that OS root, not the loader's /nix/store.

Before import/mount effects, /run/zbm-rs/managed records the selected pool GUID
and mount destinations. A restarted manager checks imported pools and mountinfo
before reusing them. It does not claim pre-existing foreign imports, force import,
or export a pool with a foreign mount. These ephemeral ownership records are
runtime state, not mutable loader configuration or a persistent recovery journal.
The first path does not create clones or change dataset properties.

## Verified-boot boundary under implementation

The complete accepted design is in [Secure Boot and verified boot](secure-boot-model.md).
The core verifier, broker, protected lifecycle and owner signing pipeline are
implemented and passed signed OVMF synthetic-handoff, real NixOS ZFS-root and
trusted snapshot-clone scenarios. Linux and
NixOS discovery produce untrusted candidates; a signed BootAuthorization binds
kernel/initramfs bytes, arguments and permitted dataset/snapshot-clone selection.

One ELF now includes a privileged broker role alongside an unprivileged manager.
Core owns trust policy and opaque verified-plan types;
the manager submits typed target/operation requests and displays evidence. The
broker independently resolves and authorizes inputs, owns privileged ZFS actions
and passes the exact prepared immutable bytes to `kexec_file_load`. Generic ZFS
mechanisms remain in zfskit. A private socket or a caller-supplied verified flag is
not authorization, and a root-capable manager could bypass Rust type boundaries.

Firmware authenticates the loader UKI. Trusted early kernel/initramfs setup then
enforces kernel/module signatures, lockdown and target-initramfs IMA appraisal;
the broker enforces the signed command-line/root policy. PID 1 must apply the same
image policy to ordinary requests, crash recovery, last-ditch errors and failed
handoff. No path automatically opens an unrestricted shell in `enforce`.

Image policy is immutable `off`/`enforce` configuration, independent of observed
firmware state. Disabling firmware Secure Boot never implicitly disables target
verification. A separate option can require firmware protection too. Runtime UI
permission for an untrusted target is deferred; no mandatory development image is
introduced. Selected root contents and rollback resistance are separate promises.

## Canonical Nix image

nix/image.nix builds the Rust ELF, chooses one kernelPackages set and derives both
kernel and ZFS modules from it. ABI and ZFS userspace/module version assertions are
explicit. A shrunk modules closure includes requested modules and matching firmware.
Default userspace is eudev plus ZFS without URL fetching; local-key native
encryption is retained, HTTPS keylocations are unavailable. Selected ELF/tools
and their interpreter/DT_NEEDED libraries are copied from Nix
paths; there is no fallback to host libraries and no whole userspace closure copy.
The copied udev rules are exposed through `/etc/udev/rules.d` as well as
`/usr/lib/udev/rules.d`: eudev's compiled vendor path points to its original Nix
prefix. The common `/etc` path makes coldplug modalias loading work after staging.
Nix builds deterministic cpio with configurable compression (default zstd-19) and systemd ukify constructs the unsigned UKI.
Runtime crates never construct cpio or EFI files. dracut/mkinitcpio are not involved.

Portable includes common storage/input and virtual-machine drivers. Host-only
requires an explicit JSON manifest or NixOS initrd module lists. The separate
read-only probe writes that manifest; derivations never inspect /sys. The probe
follows block frontend and parent transport drivers as well as PCI/USB
controllers. SCSI disks retain `sd_mod` even when it is built into the probed
kernel. Synthetic sysfs checks and an explicit host-only SCSI VM fixture cover
the controller/frontend distinction. The NixOS
module reuses boot.kernelPackages, boot.zfs.package and both initrd module lists.
The same derivation is used by local builds, CI, QEMU and system.build.zbm-rs-efi.

Security integration provides explicit loader `image.kernelPackages` selection
and `image.kernelPolicy = validate | configure`. Validation is the default;
configuration builds a separate loader variant and matching ZFS without changing
the host kernel. Final kernel capabilities, embedded certificates and module
signatures must be checked before packaging. Private signing keys stay outside
Nix; owner deployment signs an output copy. See [options](configuration.md#secure-boot-configuration)
and the trust model's build pipeline. The selected OS kernel remains an externally
produced authorized artifact, not something the boot manager rebuilds at runtime.

TPM startup only probes TPM2 and reads SHA-256 PCR15 through sysfs. The loader
never prepares SRK/NvPCRs or extends OS phases in PCR11. The target OS retains
its native initramfs/userspace TPM services and NvPCR definitions.
A bounded `systemd-pcrextend` helper from the ordinary Nix systemd package records
a target-prepared event in PCR15 after native verified loading succeeds. This
hashes the exact verified final plan and records an attempt, not successful OS
execution. Required TPM failures stop startup/handoff; optional failures never
weaken boot-input verification. The independent firmware/userspace replay test
checks unchanged PCR11, exact PCR15 and no loader NV/persistent allocation.
Transport of the loader's PCR15 log into the target initramfs remains separate
work. See [the TPM ownership/evidence guide](tpm.md),
[the trust model](secure-boot-model.md) and [the VM workflow](security-testing.md).

Production has no VM fault hooks. The test image is opt-in and compiles vm-test,
adds SSH and disposable fixture credentials (public Nix-store test data, never
host credentials). Image size is recorded and checked against checked-in policy.

## Harness and evidence

xtask owns QEMU lifecycle, QMP, SSH and reusable scenarios. Ordinary smoke exercises
real discovery and shell/poweroff requests; --lifecycle adds manager abort/panic,
dirty console restoration, clean restart, signal forwarding, orphan reaping,
CLOEXEC and deliberately leaked-descriptor coverage. Each run preserves evidence.

Serial application state, actual /dev/vcs1 text, PNGs and installed-kernel handshake
are distinct observations. The Nix image pool screenshot was visually checked and shows a complete TUI.
The older host-derived graphics discrepancy remains a dated historical observation;
state/text assertions alone do not certify the raster. VM acceptance does not
establish physical boot or Secure Boot.

## Configuration

See configuration.md: Nix generates an immutable JSON file, validated on the build
host and read once by each runtime role. Normal changes rebuild the EFI.
