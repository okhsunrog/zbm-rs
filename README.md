<div align="center">

# zbm-rs

**A Rust ZFS boot manager with Secure Boot verification, native NixOS generations, and snapshot boot.**

[![Secure Boot](https://img.shields.io/badge/Secure_Boot-Verified_boot-8957e5?logo=letsencrypt&logoColor=white)](docs/secure-boot-model.md)
[![NixOS](https://img.shields.io/badge/NixOS-Bootspec-5277c3?logo=nixos&logoColor=white)](docs/usage.md#linux-and-nixos-boot-paths)
[![TPM2](https://img.shields.io/badge/TPM2-Measured_boot-2da44e)](docs/tpm.md)

[![CI](https://github.com/okhsunrog/zbm-rs/actions/workflows/check.yml/badge.svg)](https://github.com/okhsunrog/zbm-rs/actions/workflows/check.yml)
[![Rust 2024](https://img.shields.io/badge/Rust-2024-e67e22?logo=rust)](Cargo.toml)
[![Built with Nix](https://img.shields.io/badge/Built_with-Nix-5277c3?logo=nixos&logoColor=white)](docs/build.md)
[![Early development](https://img.shields.io/badge/Status-Early_development-d4a017)](docs/roadmap.md)
[![License](https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-2ea44f)](#license)

[Secure Boot](docs/secure-boot-model.md) · [TPM](docs/tpm.md) · [Documentation](docs/index.md) · [Build an image](docs/build.md) · [Roadmap](docs/roadmap.md)

</div>

zbm-rs loads Linux from ZFS through a small Linux initramfs. Enforced images
verify the selected kernel, initramfs, arguments and root-selection policy before
handoff, including when booting an authorized snapshot clone.

Choose an ordinary Linux kernel or a native NixOS Bootspec generation, inspect
its boot inputs, and start a writable snapshot clone without rolling back the
original dataset.

> **Early development.** Protected NixOS root and snapshot-clone boot have passed
> OVMF Secure Boot tests. Physical Secure Boot acceptance, generic-Linux verified
> boot and interactive encrypted-root unlock are still pending. Default builds
> are unsigned and use `security.mode = "off"`. See [tested scope](docs/verification.md).

## Secure Boot and verified boot

Firmware Secure Boot authenticates the signed loader image. An enforced loader
then authorizes the OS boot inputs as one combination:

- **Kernel and initramfs:** owner-authorized bytes, with kernel signature checking
  and IMA appraisal of the initramfs.
- **Arguments and ZFS root:** a signed BootAuthorization fixes the command line
  and defines which root or owned snapshot clone may be selected.
- **Protected execution and recovery:** an unprivileged interface submits requests
  to a privileged broker. Failed checks refuse that boot attempt; recovery offers
  diagnostics, restart, reboot and poweroff.

Image policy is fixed at build time. Disabling firmware Secure Boot does not
turn off an enforced image's target checks. Public certificates are image inputs;
private signing keys stay outside Nix and image outputs. Boot-input verification
does not authenticate every file in the selected root or prevent rollback.

Read the [trust-chain design](docs/secure-boot-model.md) and
[owner signing workflow](docs/security-testing.md).

## Features

| | Capability |
| --- | --- |
| 🔐 **Secure Boot and verified boot** | Authorize the kernel/initramfs/argument combination and permitted ZFS root, including trusted snapshot clones. |
| ❄️ **Native NixOS generations** | Read Bootspec directly and select the generation's system closure, even when generations share a kernel. |
| 🌿 **ZFS environments and snapshots** | Browse Linux roots and snapshots. Prepare an owned writable boot clone; ordinary mode also supports confirmed rollback, persistent clones and promotion. |
| 📏 **TPM2 measured boot** | Optionally record the verified prepared target and final arguments in PCR15; leave SRK/NvPCR setup to the selected OS. |
| 🧱 **Declarative Nix images** | Build the loader kernel, matching ZFS, userspace, configuration and public trust inputs together, with portable or explicit hardware profiles. |
| ⌨️ **Console UI and recovery** | Search lists, inspect targets and use help or the action menu. A separate PID 1 supervisor restores the console after manager failure. |

[![NixOS generation selection in zbm-rs](docs/screenshots/generations.png)](docs/screenshots/generations.png)

*The running interface in QEMU, browsing two installed NixOS generations in ordinary mode.*

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

These are unmodified captures of a disposable VM. See
[the screenshot setup](docs/screenshots/README.md).

## Relationship to ZFSBootMenu

[ZFSBootMenu](https://docs.zfsbootmenu.org/en/latest/) established the Linux/initramfs
and kexec boot model used here. zbm-rs preserves familiar ZFSBootMenu dataset
properties and uses [zfskit](https://github.com/okhsunrog/zfskit) for ZFS operations.
Its focus is integrated boot-input authorization, native NixOS generations and
an explicit supervisor/manager/broker boundary.

ZFSBootMenu already supports EFI signing, snapshot management and QEMU testing;
its encryption, recovery and distribution support are broader today. zbm-rs is
an independent implementation in early development, with [specific supported
layouts](docs/usage.md#supported-layouts) rather than full feature parity.

## TPM measured boot

With TPM support enabled, the verified executor measures the prepared boot plan
in PCR15 after successful input loading. This records preparation, not successful
OS execution. Firmware/stub measurement of the loader and OS-owned SRK/NvPCR setup
remain separate. TPM disk unlock, remote attestation, rollback resistance and
PCR15 log transport into the selected OS are not implemented. See [TPM ownership
and evidence](docs/tpm.md).

## Build and try it

On an x86_64 Linux host with Nix and flakes enabled:

```sh
nix build .#zbm-rs-efi
```

The result includes `result/esp/EFI/BOOT/BOOTX64.EFI`, the loader kernel, initramfs
and a build manifest. The NixOS module exposes an image without replacing your
existing bootloader. Configuration changes rebuild the image; protected images
use the separate owner signing workflow.

For a disposable QEMU session:

```sh
nix build .#zbm-rs-efi-test --out-link result-test
cargo xtask vm --run target/vm/demo-001 boot --image result-test
```

The test image contains SSH and harness controls. Use fresh run directories and
disposable guest disks; keep this profile out of normal deployments. The
[build guide](docs/build.md) covers profiles and NixOS integration, and the
[development guide](docs/development.md) covers fixtures and VM controls.

## Documentation

| Guide | Contents |
| --- | --- |
| [Secure Boot](docs/secure-boot-model.md) | Trust boundaries, authorization, key roles and protected recovery. |
| [Signing and security tests](docs/security-testing.md) | Owner publishing and disposable signed OVMF/TPM scenarios. |
| [TPM](docs/tpm.md) | Measurement policy, OS ownership and evidence. |
| [Usage](docs/usage.md) | Boot layouts, snapshots and keyboard controls. |
| [Building and configuration](docs/build.md) | Nix images and [immutable settings](docs/configuration.md). |
| [Architecture and development](docs/architecture.md) | Runtime roles and the [VM harness](docs/development.md). |
| [Verification](docs/verification.md) | Tested boundaries and remaining acceptance gates. |

The [documentation index](docs/index.md) also links research, historical records
and the roadmap.

## License

Licensed under either [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.
