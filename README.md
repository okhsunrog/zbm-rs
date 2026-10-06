# zbm-rs

A Rust ZFS boot manager with a canonical Nix image and a persistent QEMU harness.
The initramfs contains one main ELF: /bin/zbm-rs, with /init -> /bin/zbm-rs.

PID 1 runs a synchronous std/libc supervisor. It starts the same executable as a
manager child; the manager owns Tokio, zfskit and Ratatui. Manager failure never
terminates PID 1. The parent restores the console, restarts once after a rapid
failure, then uses emergency recovery instead of an unlimited crash loop.

The installed-OS paths discover ordinary Linux kernels/initramfs and NixOS
Bootspec generations on selected ZFS filesystems, then boot through
kexec_file_load. Encryption unlock UX, specialisations and Secure Boot acceptance
remain roadmap work.

## Why this project exists

ZFSBootMenu established a useful model: boot a small Linux environment, discover
ZFS boot environments and hand a selected kernel over to kexec. zbm-rs explores
that model with Rust, explicit process boundaries and a reproducible Nix image.
It is a new implementation inspired by ZFSBootMenu, not a drop-in replacement yet.

The project grows out of work on [zfskit](https://github.com/okhsunrog/zfskit) and
[archinstall_zfs](https://github.com/okhsunrog/archinstall_zfs): reuse the tested ZFS
command layer and console-supervisor ideas, and make real boot testing part of
development from the beginning. A persistent, agent-friendly QEMU harness lets
humans and automation operate the same VM through keys, screens, logs and SSH.

## Goals

- Discover and boot generic Linux ZFS boot environments using kernel/initrd discovery and kexec.
- Support NixOS generations, Bootspec and specialisations as an explicit backend.
- Provide safe snapshot boot through temporary clones, with clear recovery and encryption flows.
- Preserve useful ZFSBootMenu on-disk conventions where compatibility is practical.
- Keep PID 1 alive when the manager fails, and restore a usable recovery console.
- Produce an immutable EFI image through one Nix build, with a matching kernel/ZFS pair,
  portable and explicit-manifest host-only profiles, and measurable image size.
- Grow deterministic process, UI and ZFS integration tests alongside every boot feature.

The working foundation includes discovery, the supervisor/manager split,
immutable JSON configuration and the Nix/QEMU lifecycle harness, plus an initial
Linux and NixOS boot paths, snapshot clone boot, persistent clones/promotion and
confirmed single-dataset rollback. Encryption UX remains roadmap work.

Cargo and Nix use the published `zfskit 0.3` crate from crates.io, pinned by
`Cargo.lock`. A sibling zfskit checkout is not required.

## Linux and NixOS boot paths

Enter on a pool explicitly imports it without force and discovers boot environments.
The list follows ZFSBootMenu visibility: `mountpoint=/` unless
`org.zfsbootmenu:active=off`, or `mountpoint=legacy` with `active=on` (including
inherited values). `bootfs` marks and selects the default BE; it never triggers
automatic boot. Eligible unencrypted datasets are mounted read-only; hidden and
non-root datasets are not mounted. Encrypted or `canmount=off` candidates remain
visible with a diagnostic. Enter on a BE opens its Linux kernels or NixOS generations; B goes back.
An individual mount or Bootspec failure leaves other BEs and generations
available. Rejected generations retain their generation number and diagnostic.
Enter on a generation resolves its Bootspec,
loads its kernel/initrd, unmounts the roots owned by this manager and exports its
owned pools before executing kexec. Only pools imported by zbm-rs are eligible for export;
restart reconciliation checks actual pool GUIDs and existing mountpoints.

The Linux backend follows ZBM kernel/initramfs naming in `/boot`, naturally sorts
versions, honors `org.zfsbootmenu:kernel`, and reads `commandline` (including
`%{parent}` expansion) or a per-kernel `.kcl` file. `rootprefix` is explicit or
inferred from `ID`/`ID_LIKE` without executing os-release. The loader suppresses
conflicting `root=`/`zfs=` arguments and names the selected dataset, including
clones. Existing initramfs images are used unchanged. Paths and matching pairs
are checked again before handoff; ZFS properties are freshly read.

The NixOS backend assumes that /nix/store and /nix/var/nix/profiles are in the
selected root filesystem. Separate /nix datasets, encrypted roots, initrdSecrets,
specialisations and kernel arguments requiring whitespace/quoting are unsupported.
A configured root different from the selected dataset requires an automatically
recognized systemd initrd for root override; scripted NixOS initrd root override
is unsupported. No target initrd is rewritten.
Discovery itself remains non-mutating. Boot actions are unavailable in local
preview mode. Autoboot is not implemented.

Linux ZBM properties do not override NixOS Bootspec. Pool
`org.zfsbootmenu:readonly` and the image import policy govern mutations.
Encrypted BE unlock/keysource, duplicate via send/receive and some other ZBM
features are still unimplemented; this is not full ZBM feature parity.

## Snapshots and recovery

From the BE list, T opens snapshots when `settings.ui.showSnapshots` is enabled.
Enter inspects a snapshot's Linux targets or NixOS generations read-only.
C prepares an owned boot clone, D discards a prepared clone, and Enter on a target
prepares/reuses a clone and boots it. Snapshot listings are capped at the newest
256 per BE.

O creates a persistent ordinary boot environment from the selected snapshot;
M creates and promotes it, after confirmation. These operations do not require
a discoverable boot target or NixOS metadata. The clone is selected in the normal
BE list; its boot properties are preserved and pool bootfs is unchanged.
U rolls the original dataset back to the selected snapshot, after typing
`ROLLBACK`. This discards current changes and newer snapshots (`zfs rollback -r`),
but never force-destroys dependent clones or recursively rolls back child datasets.
Owned inspection mounts are removed first; foreign mounts or read-only policy
block the operation. Boot targets are rediscovered afterward.

No installed-system flag or custom Bootspec extension is required. Linux clone
boot uses the selected dataset via the native root prefix. NixOS clone boot
inspects the actual `/init` in a bounded newc initramfs (gzip, xz or zstd) and
requires systemd initrd, with the Nix store in the root dataset; additional initrd
mounts such as a separate /usr remain unsupported. The legacy
`programs.zbm-rs.snapshotBoot.enable` option is a compatibility no-op.
Rollback and persistent clone/promotion do not depend on this initrd check.

Mutations require a writable owned pool import. Read-only policy is never
silently upgraded; `org.zfsbootmenu:readonly` also forbids mutations.
Prepared boot clones are separate from ordinary persistent BEs.
Prepared clones have `canmount=noauto`, `active=off` and an ownership token.
Linux boot clones use `mountpoint=/` for native initramfs compatibility; NixOS
boot clones use `mountpoint=legacy` with the explicit systemd root override. An ephemeral `/run` journal records intent before creation and verifies
the token, origin and snapshot GUID before restart reuse or explicit discard.
Discard uses non-recursive destruction and refuses mounted or retained clones.

Clones are marked `org.zbm-rs:state=retained` before handoff. They remain after
boot, reboot or a failed handoff, preserving writes from the target OS. They
are hidden from ordinary BE discovery and are never automatically deleted.
The user can inspect their origin and ownership properties and manage retained
clones with ZFS tooling. Reuse of prepared clones is currently limited to
manager restarts within the same loader boot; cross-reboot cleanup/adoption
is intentionally deferred.

```sh
nix build .#zbm-rs-efi-test-snapshot --out-link result-snapshot
nix build .#zbm-rs-boot-fixture --out-link result-fixture
cargo xtask boot-smoke --snapshot --image result-snapshot --fixture result-fixture --run target/vm/snapshot-new
```

Arch acceptance uses the actual sanitized mkarchiso staging tree from
`archinstall_zfs`, a real ZFS-root mkinitcpio image and a disposable VM disk:

```sh
uv run --no-project python xtask/fixtures/build_arch.py \
  ~/code/archinstall_zfs/gen_iso/workdir/x86_64/airootfs \
  target/arch-fixture-new --sudo
cargo xtask arch-smoke --image result-snapshot --fixture target/arch-fixture-new \
  --run target/vm/arch-live-new --mode live
```

Use fresh run directories with `--mode snapshot`, `rollback`, `clone` or
`promote` for the other paths. The reviewed fixture builder currently pins
Linux LTS 6.18.53-1-lts. This is a staged-BE boot test, not a full installer-wizard
run. Rollback checks cancellation, modified Enter, dependent-clone protection
and read-only policy before restoring data and booting the result.

The persistent boot acceptance fixture contains two real NixOS generations:

```sh
nix build .#zbm-rs-efi-test --out-link result-test
nix build .#zbm-rs-boot-fixture --out-link result-fixture
cargo xtask boot-smoke --image result-test --fixture result-fixture --run target/vm/nixos-new --tcg
```

The harness installs the generated closure from a read-only fixture disk onto a
fresh disposable ZFS disk. It restarts the manager after import/mount, selects the
older generation using the real keyboard, and requires a target-OS success marker
with the exact current-system, root dataset and a new boot ID, then poweroff.
See [the handoff trust model](docs/secure-boot-model.md) for security boundaries.

## Build

Enable the Nix nix-command/flakes features, then:

```sh
nix build .#zbm-rs-efi
nix build .#zbm-rs-efi-test --out-link result-test
```

On hosts with those features disabled, prefix commands with
`nix --extra-experimental-features 'nix-command flakes'`.
The production result contains the complete unsigned EFI artifact, kernel,
initramfs, manifest and image-size measurements:

```text
result/esp/EFI/BOOT/BOOTX64.EFI
result/vmlinuz
result/initramfs.img
result/manifest.json
result/sizes.json
```

Nix builds Rust from the locked sources, selects kernel and ZFS modules from one
kernelPackages set, checks their ABI/version pairing, includes selected tools and
ELF dependencies, creates cpio/zstd and invokes systemd ukify. Normal builds use
neither dracut nor mkinitcpio nor /boot from the development host. No image-building
code runs inside zbm-rs. `cargo xtask image [--test-ssh]` delegates to this same
Nix derivation.

The test image compiles the opt-in vm-test feature and adds loopback-forwarded SSH,
private test controls and disposable fixture credentials. Its key is public
Nix-store test data, not a security credential; never use this profile as a normal
boot image or with persistent sensitive guest data. Production omits those hooks.

## Profiles and NixOS

Portable includes common storage/input and QEMU drivers. Host-only takes explicit
module/firmware inputs. Probe outside Nix, inspect the JSON, then pass it to mkImage:

```sh
uv run --no-project nix/probe-hardware.py hardware-manifest.json
```

`lib.mkImage { profile = "host-only"; hardwareManifest = ./hardware-manifest.json; }`
is the reusable build API for a consuming flake. Commit the reviewed manifest with
the caller's configuration; derivations never read /sys. Module names must exist
in the selected kernel, so incompatible manifests fail rather than being ignored.
`zbm-rs-efi-host-only-example` is explicitly a QEMU fixture, not a probe of your host.

Import `nixosModules.default`, enable `programs.zbm-rs.enable` and select its
`image.profile`. `system.build.zbm-rs-efi` then uses the same builder with
`boot.kernelPackages`, `boot.zfs.package`, `boot.initrd.availableKernelModules`
and `boot.initrd.kernelModules`. The module exposes an artifact; it does not
silently replace the machine's existing bootloader.

Image composition lives under `programs.zbm-rs.image`; immutable runtime policy
lives under `programs.zbm-rs.settings`, mirroring the Rust/JSON hierarchy:

```nix
programs.zbm-rs = {
  enable = true;
  image.profile = "portable";
  settings = {
    ui.timeout = 5; # Reserved until autoboot is implemented.
    manager.restartLimit = 2;
    zfs.importPolicy = "read-only";
    nixos.generationLimit = 20;
  };
};
```

Previous flat option names remain deprecated aliases. The module-config Nix check
compares generated defaults with Rust defaults, validates both range endpoints,
and checks that legacy aliases produce the same JSON.

## Development and VM tests

```sh
cargo fmt --all --check
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo run -p zbm-rs -- --manager --preview
cargo xtask smoke --run target/vm/smoke-001
cargo xtask smoke --run target/vm/lifecycle-001 --lifecycle
```

Build result-test first. Every run uses a fresh folder and its own disposable disk.
Smoke owns and reaps QEMU even on failure/interruption. Add `--failures` to
exercise isolated mount/Bootspec failures, Shell during a blocked mount,
operation timeout/restart reconciliation, and Power off during a blocked mount.
The lifecycle variant
also checks abort/panic/SIGSEGV, actual terminal damage/restoration, manager
restart, PID-1 signal forwarding, adopted-child reaping, CLOEXEC, deliberately
leaked descriptors and failed-kexec classification. The separate `boot-smoke`
scenario proves successful handoff into the selected installed generation.

The explicit host-only SCSI fixture exercises the disk frontend omitted by a
controller-only hardware manifest:

```sh
nix build .#zbm-rs-efi-test-host-only-scsi --out-link result-scsi
cargo xtask smoke --image result-scsi --run target/vm/scsi-new --port 2231 --scsi
```

Interactive boot stays in a tracked foreground terminal/session. Other controls
run from another terminal:

```sh
cargo xtask vm --run target/vm/dev-001 boot
cargo xtask vm --run target/vm/dev-001 status
cargo xtask vm --run target/vm/dev-001 screen
cargo xtask vm --run target/vm/dev-001 screen --text
cargo xtask vm --run target/vm/dev-001 key r
cargo xtask vm --run target/vm/dev-001 screenshot target/vm/dev-001/screen.png
cargo xtask vm --run target/vm/dev-001 logs
cargo xtask vm --run target/vm/dev-001 ssh 'cat /run/zbm-rs/supervisor.log'
cargo xtask vm --run target/vm/dev-001 stop
```

Boot defaults to KVM/OVMF, with matched firmware paths on Arch/Debian/Ubuntu.
Use --tcg without KVM, --direct to skip EFI during Linux iteration, --image to
select another Nix result, and --port to change the loopback SSH port (2228).
SSH commands have a 30-second guest and 35-second host deadline.

R rescans, S asks PID 1 for a shell, N requests a clean restart, P asks PID 1 to
power off. Exiting the supervised shell starts a fresh manager with coherent ZFS
state. Q exits manager; without an intent PID 1 treats that as unexpected exit.
Outside supervision S runs a local shell and Q exits normally.
Boot operations run individually while recovery keys remain available. A
30-second operation timeout cancels the async command and requires Shell or
Restart before further boot actions; the next manager reconciles recorded
resource intent. Blocking OS-root reads run outside the input loop, and shutdown
does not wait indefinitely for those reads.

The TUI uses a candidate list and a read-only details pane on wide consoles
(110 columns or more). Smaller consoles keep the list and recovery controls;
I/F3 opens the same complete details in a scrollable panel. Bootspec previews
show the generation's actual kernel, initrd, init and arguments. Execution still
revalidates the boot inputs. Rejected generations can be inspected but cannot boot.

- `/` starts fuzzy search in the current list. Enter accepts the filter without
  booting; Esc clears it. Filters and selection survive navigation back to a list.
- F1/`?` shows keyboard help; F2 opens all actions, including unavailable actions
  with their reasons. Both use the same command inventory as dispatch.
- Existing Enter/B/T/C/D/R/S/N/P/Q bindings remain available outside search.
  F4 Shell, F5 Rescan, F6 Restart manager and F10 Power off provide function-key
  alternatives; recovery remains available during search and pending operations.
- D opens a confirmation before discarding a prepared owned clone. Esc cancels;
  Enter confirms. Clone ownership and non-recursive destruction stay in core.

N restarts only the manager; Q exits it and leaves recovery to PID 1. These actions
do not reboot the machine. The interface does not yet provide chroot, full pool
status, kernel-argument editing, diff or other unimplemented ZBM actions.

`screen` returns instrumented application state. `screen --text` reads the actual
VT through /dev/vcs1 (ASCII labels, font glyphs replaced with spaces). PNG capture
is separate evidence. The new Nix image has a visually checked complete TUI.
The old host-derived image showed a raster/text discrepancy
after Rescan; logs and text checks must not be treated as visual acceptance.

## Size experiments

Release builds use `opt-level="z"`, `codegen-units=1` and FatLTO with panic
unwinding. The default image uses zstd-19 and the eudev and ZFS userspace without optional URL fetching.
Standalone-linked modern udev remains an experimental comparison profile. `lib.mkImage` also accepts
`initramfsCompression = "gzip" | "xz" | "zstd"` and a `rustProfile` attrset for
controlled comparisons. See [measured size experiments](docs/size-experiments.md).

Native encryption with local keys is retained. **HTTPS encryption keylocations
are unavailable in the default image.** Normal configuration and NixOS builds
use this same userspace. The previous `-lean` names are aliases for the defaults.

`zbm-rs-efi-full-userspace` and `zbm-rs-efi-test-full-userspace` retain upstream
URL-fetch support and systemdMinimal for explicit comparisons or URL-key needs.

## Whole-userspace musl experiment

`zbm-rs-efi-musl` and `zbm-rs-efi-test-musl` target musl for the entire initramfs
userspace: Rust, BusyBox, ZFS, eudev, kmod and their libraries. Native build tools
and the matching kernel/ZFS module pair remain separate from that runtime package
set. The staged ELF audit rejects glibc dependencies in musl images.
See [the experiment](docs/musl-experiment.md) for build and acceptance results.
The normal outputs continue to use glibc until an explicit default change.

## Size regression policy

Every build writes sizes.json. nix/image-size-policy.json records initial
production/test baselines, a 10% growth allowance plus 2 MiB, and absolute limits.
A build fails when unexplained growth exceeds policy. Update the baseline only
with a reviewed explanation. Host-only has an absolute cap; its hardware-specific
baseline should be recorded by its consuming configuration.

CI builds production and test through this same Nix path, checks size policy and
runs the lifecycle scenario with TCG. VM acceptance does not establish physical
boot or Secure Boot. See docs/architecture.md and dated verification reports.

## Image configuration

Nix generates immutable JSON; one Rust schema validates it at build time and
runtime. See docs/configuration.md for defaults, Nix options and development
overrides. No const-gen, custom macro, config writer or reload service is used.
