# Architecture

## One ELF, two processes

The image contains /bin/zbm-rs and /init -> /bin/zbm-rs. main checks PID 1 before
argument parsing, Tokio, Ratatui or ZFS initialization. PID 1 runs supervisor;
it spawns current_exe() as a fresh manager generation. Outside initramfs the
normal entry is manager; --manager --preview is explicit development mode.

The supervisor lifecycle uses std/libc and the small private protocol. It also
reads the image-owned typed JSON once through the shared config schema; it never
initializes Tokio, Ratatui or ZFS application state.
It mounts proc/sys/dev/run/tmp/devpts, runs the image's driver/udev bootstrap child,
saves tty1 state and supervises managers and emergency shells. The manager owns
ZFS initialization, Tokio, zfskit, pool discovery and the Ratatui event loop.
There is no zbm-init artifact and no shell/systemd/getty underneath PID 1.

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

## Canonical Nix image

nix/image.nix builds the Rust ELF, chooses one kernelPackages set and derives both
kernel and ZFS modules from it. ABI and ZFS userspace/module version assertions are
explicit. A shrunk modules closure includes requested modules and matching firmware.
Default userspace is eudev plus ZFS without URL fetching; local-key native
encryption is retained, HTTPS keylocations are unavailable. Selected ELF/tools
and their interpreter/DT_NEEDED libraries are copied from Nix
paths; there is no fallback to host libraries and no whole userspace closure copy.
Nix builds deterministic cpio with configurable compression (default zstd-19) and systemd ukify constructs the unsigned UKI.
Runtime crates never construct cpio or EFI files. dracut/mkinitcpio are not involved.

Portable includes common storage/input and virtual-machine drivers. Host-only
requires an explicit JSON manifest or NixOS initrd module lists. The separate
read-only probe writes that manifest; derivations never inspect /sys. The NixOS
module reuses boot.kernelPackages, boot.zfs.package and both initrd module lists.
The same derivation is used by local builds, CI, QEMU and system.build.zbm-rs-efi.

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
