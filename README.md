# zbm-rs

A Rust ZFS boot manager with a canonical Nix image and a persistent QEMU harness.
The initramfs contains one main ELF: /bin/zbm-rs, with /init -> /bin/zbm-rs.

PID 1 runs a synchronous std/libc supervisor. It starts the same executable as a
manager child; the manager owns Tokio, zfskit and Ratatui. Manager failure never
terminates PID 1. The parent restores the console, restarts once after a rapid
failure, then uses emergency recovery instead of an unlimited crash loop.

Installed-OS selection/kexec, BE discovery, encryption unlock, snapshot boot,
NixOS generation discovery and Secure Boot are not implemented yet.

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

The working foundation today is discovery, the supervisor/manager split,
immutable JSON configuration and the Nix/QEMU lifecycle harness. The boot targets,
NixOS backend, snapshot boot and encryption goals above are roadmap work.

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
profile. `system.build.zbm-rs-efi` then uses the same builder with
`boot.kernelPackages`, `boot.zfs.package`, `boot.initrd.availableKernelModules`
and `boot.initrd.kernelModules`. The module exposes an artifact; it does not
silently replace the machine's existing bootloader.

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
Smoke owns and reaps QEMU even on failure/interruption. The lifecycle variant
also checks abort/panic/SIGSEGV, actual terminal damage/restoration, manager
restart, PID-1 signal forwarding, adopted-child reaping, CLOEXEC, deliberately
leaked descriptors and failed-kexec classification. Successful kernel handoff
will need a later scenario once kexec exists.

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
