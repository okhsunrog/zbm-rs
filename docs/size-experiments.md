# Image size experiments

Source baseline: `fa3f9ff2a677e1e3b4762bfa16ced83a889a00f1`; host compiler: `rustc 1.98.1 (48a229cea 2026-09-01)`.

The Rust matrix uses identical sources and Cargo.lock. Each variant uses strip,
panic unwinding, and the settings below; `--print-config` is checked after linking.
Times are single-run wall-clock measurements, not a controlled latency benchmark.
Nix uses its pinned compiler, so host ELF sizes are not interchangeable with Nix ELF sizes.

| Profile | Opt | LTO | CGU | ELF bytes | gzip bytes | Build seconds |
| --- | --- | --- | --- | ---: | ---: | ---: |
| baseline | 3 | thin | 16 | 3031584 | 1156865 | 12.881 |
| thin-cgu1 | 3 | thin | 1 | 2652304 | 1063401 | 16.652 |
| fat-cgu1 | 3 | fat | 1 | 2498912 | 1023738 | 29.371 |
| s-fat-cgu1 | s | fat | 1 | 2098264 | 857466 | 19.594 |
| z-fat-cgu1 | z | fat | 1 | 2040016 | 825948 | 18.072 |

Compression input: the exact same production cpio, 40734208 bytes.
Every encoded result was decompressed and compared byte for byte. zstd uses one thread.

| Compressor | Bytes | Compress seconds | Host decode seconds |
| --- | ---: | ---: | ---: |
| gzip-9 | 17804600 | 5.579 | 0.148 |
| xz-6 | 13972472 | 13.795 | 0.576 |
| zstd-3 | 17782453 | 0.202 | 0.068 |
| zstd-10 | 16580100 | 0.876 | 0.08 |
| zstd-15 | 16413836 | 6.728 | 0.065 |
| zstd-19 | 14916707 | 21.887 | 0.09 |

## Userspace research

[Gentoo ZFS ebuild](https://github.com/gentoo/gentoo/blob/master/sys-fs/zfs/zfs-2.4.4.ebuild)
uses `minimal` to omit Python tools, and separately controls PAM, NLS and unwind.
Our base build already has `enablePython=false`, and stages selected ELF dependencies
rather than whole package closures. Docs, services and tests are not in the image.

[Gentoo systemd-utils](https://github.com/gentoo/gentoo/blob/master/sys-apps/systemd-utils/systemd-utils-262.ebuild)
builds selected utilities with `--auto-features=disabled`. Modern Gentoo uses this
instead of eudev. Nix systemdMinimal already disables most optional integrations.

[Systemd udev linking](https://github.com/systemd/systemd/blob/v261.3/src/udev/meson.build)
supports `link-udev-shared=false`. This keeps the modern daemon while avoiding a
full libsystemd-shared dependency. The standalone-linked profile is a build experiment.

The pinned Nix ZFS recipe forces libcurl link dependencies. Upstream
[libfetch detection](https://github.com/openzfs/zfs/blob/zfs-2.4.4/config/user-libfetch.m4)
is optional. The adopted default omits curl/PAM/NLS, retaining OpenSSL and native
ZFS encryption. HTTPS keylocations are unavailable; this is an explicit image
policy. Eudev retains kmod and storage rules/helpers. Full userspace is available
through the named comparison outputs.

## Reproduction

```sh
uv run --no-project nix/benchmark-size.py rust
uv run --no-project nix/benchmark-size.py compression
nix build .#zbm-rs-efi-test-zstd --out-link target/image-zstd
nix build .#zbm-rs-efi-test-standalone-udev --out-link target/image-static-udev
nix build .#zbm-rs-efi-test-lean --out-link target/image-lean
cargo xtask smoke --image target/image-zstd --run target/vm/zstd-new --lifecycle
```

## Nix image validation

The default is Rust opt-level z/FatLTO/CGU1, zstd-19, eudev and
ZFS without optional URL fetching. Panic unwinding remains enabled for PID-1 recovery.

| Image | EFI bytes | initramfs bytes | Acceptance |
| --- | ---: | ---: | --- |
| Original production, gzip/thin/default CGU | 31821312 | 17804600 | full UEFI/TCG lifecycle |
| Optimized production, zstd/standard userspace | 28632576 | 14615776 | matching test profile below |
| Optimized test, zstd/standard userspace | 29705728 | 15688914 | full UEFI lifecycle |
| Standalone-linked udev test | 29563392 | 15546855 | full UEFI lifecycle |
| Lean eudev/no-fetch ZFS production | 24410112 | 10393175 | matching test profile below |
| Lean eudev/no-fetch ZFS test | 25980928 | 11964376 | full UEFI lifecycle |
| Optimized XZ test | 28715008 | 14698216 | UEFI discovery/encryption/shell/poweroff smoke |

Full scenarios include native encrypted dataset creation, unload/load of a local
passphrase key, snapshot/clone/rename/promote/export/discovery, terminal input,
PID-1 crash isolation, console restoration, signals, reaping, IPC FD discipline
and crash-loop recovery. XZ's smoke covers ordinary boot operations, not the entire
fault matrix. The actual kernel accepted both zstd and XZ initramfs formats.

During experiments, shared-serial writers exposed fragmented JSON records. Manager
and supervisor now construct complete records before writing; JSON has a leading
separator to recover from a preceding partial log line. The harness also re-execs
/proc/self/exe, preserving its running inode across concurrent Cargo relinking.
These corrections were validated in the complete reruns rather than treating a
smaller binary as sufficient acceptance.

The standalone-link experiment removes the runtime DSO from the staged closure,
but grows udevadm from 938,408 to 4,243,944 bytes, ata_id from 29,256 to 2,482,456
and scsi_id from 45,976 to 2,494,744. Its test EFI was only 142,336 bytes smaller
than the equivalent zstd/Rust profile with standard udev. It also needs a custom
systemd build. This is not a 6-MiB image saving. The adopted default uses eudev instead.

The lean userspace was adopted as the default on 2026-10-06. Production/test
regression baselines are now 24,410,112 / 25,980,928 bytes; growth allowances and
absolute caps are unchanged. The full-userspace named outputs preserve the old
ZFS URL-fetch capability for explicit use.
