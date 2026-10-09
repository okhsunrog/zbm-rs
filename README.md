<div align="center">

# zbm-rs

**A Rust boot manager for Linux on ZFS.**

[![CI](https://github.com/okhsunrog/zbm-rs/actions/workflows/check.yml/badge.svg)](https://github.com/okhsunrog/zbm-rs/actions/workflows/check.yml)
[![Rust 2024](https://img.shields.io/badge/Rust-2024-e67e22?logo=rust)](Cargo.toml)
[![Built with Nix](https://img.shields.io/badge/Built_with-Nix-5277c3?logo=nixos&logoColor=white)](docs/build.md)
[![License](https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-2ea44f)](#license)
[![Early development](https://img.shields.io/badge/Status-Early_development-d4a017)](docs/roadmap.md)

[Documentation](docs/index.md) · [Build an image](docs/build.md) · [Verified boot](docs/secure-boot-model.md) · [Roadmap](docs/roadmap.md)

</div>

Choose a ZFS boot environment, a Linux kernel or a NixOS generation, and boot it
from a small Linux initramfs. Browse snapshots and start a writable clone without
rolling back the original dataset.

Inspired by ZFSBootMenu, zbm-rs combines a keyboard-driven terminal interface,
reproducible Nix images and an owner-controlled verified boot path. ZFS operations
use [zfskit](https://github.com/okhsunrog/zfskit).

[![NixOS generation selection in zbm-rs](docs/screenshots/generations.png)](docs/screenshots/generations.png)

*The running interface in QEMU, browsing two installed NixOS generations.*

> **Early development.** Ordinary Linux/NixOS boot and protected NixOS boot have
> VM coverage. Interactive encrypted-root unlock, NixOS specialisations and
> physical Secure Boot acceptance are still pending. See the
> [supported scope](docs/verification.md) before using it as your primary loader.

## What it does

| | Capability |
| --- | --- |
| 🗂️ **Boot environments** | Browse ZFS roots using familiar ZFSBootMenu dataset properties. Import pools explicitly and inspect targets before booting. |
| 🐧 **Linux and NixOS** | Discover kernel/initramfs pairs in `/boot` or select a NixOS Bootspec generation with its own system closure. |
| 🌿 **Snapshots** | Inspect a snapshot and boot an owned writable clone. Ordinary mode also offers confirmed rollback, persistent clones and promotion. |
| ⌨️ **Terminal UI** | Search lists, inspect boot details and use keyboard help or the action menu. Wide and compact consoles share the same controls. |
| 🔐 **Verified boot** | Enforce owner signatures for boot artifacts, exact arguments and permitted ZFS root selection, including authorized snapshot clones. |
| 🧰 **Recovery and testing** | A separate PID 1 supervisor keeps recovery available after manager failure. A persistent QEMU harness exercises real boot and failure paths. |

## A closer look

<table>
<tr>
<td width="50%">
<a href="docs/screenshots/snapshot.png"><img src="docs/screenshots/snapshot.png" alt="Snapshot generation with an owned writable boot clone prepared"></a>
<p><strong>Snapshot boot</strong><br>Inspect the source snapshot, prepared clone and final root arguments.</p>
</td>
<td width="50%">
<a href="docs/screenshots/keyboard-help.png"><img src="docs/screenshots/keyboard-help.png" alt="Keyboard help in the running zbm-rs interface"></a>
<p><strong>Keyboard controls</strong><br>Navigation, inspection and recovery actions are available from the console.</p>
</td>
</tr>
</table>

Screenshots are unmodified captures of a disposable VM. See
[their setup and reproduction steps](docs/screenshots/README.md).

## Build and try it

Build the unsigned EFI image on an x86_64 Linux host with Nix and flakes enabled:

```sh
nix build .#zbm-rs-efi
```

The output includes `result/esp/EFI/BOOT/BOOTX64.EFI`, the loader kernel,
initramfs and a build manifest. Portable and explicit hardware profiles use the
same builder; the NixOS module exposes an image without replacing your existing
bootloader. Configuration changes rebuild the image.

For a disposable QEMU session with the development harness:

```sh
nix build .#zbm-rs-efi-test --out-link result-test
cargo xtask vm --run target/vm/demo-001 boot --image result-test
```

The test image contains SSH and test controls. Use a fresh run directory and
disposable guest disks; keep this profile out of normal deployments.
Read the [build guide](docs/build.md) for image outputs and NixOS integration,
or [the development guide](docs/development.md) for fixtures and VM controls.

## Verified boot and TPM

An enforced image checks the selected kernel, initramfs, command line and root
policy before handing control to the OS. Firmware Secure Boot authenticates the
loader image; the loader kernel and privileged broker verify the next boot inputs.
The interface runs without root privileges, and protected recovery offers
diagnostics, restart, reboot and poweroff.

Policy is selected when building the image. Turning off firmware Secure Boot
does not disable an image's target verification. Public trust inputs belong in
the image; owner signing keys stay outside Nix and image outputs. Default builds
are unsigned and use `security.mode = "off"`.

Optional TPM support records the verified prepared target in PCR15. The selected
OS owns SRK/NvPCR setup and OS phase measurements. This records preparation;
it does not provide disk unlock, remote attestation or rollback prevention.

See the [trust-chain design](docs/secure-boot-model.md),
[signing workflow](docs/security-testing.md) and [TPM guide](docs/tpm.md).

## Documentation

| Guide | Contents |
| --- | --- |
| [Usage](docs/usage.md) | Boot layouts, snapshot ownership and keyboard controls. |
| [Building](docs/build.md) | Nix images, hardware profiles and NixOS integration. |
| [Configuration](docs/configuration.md) | Immutable image settings and security options. |
| [Architecture](docs/architecture.md) | Supervisor, manager, broker and boot backends. |
| [Development](docs/development.md) | Local checks and the persistent VM harness. |
| [Verification](docs/verification.md) | Tested boundaries and remaining acceptance gates. |

The [documentation index](docs/index.md) also links research, historical test
records and the roadmap.

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.
