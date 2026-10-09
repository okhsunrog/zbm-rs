# Whole-initramfs musl experiment

These measurements describe the builds and revisions recorded below. For current
image outputs and defaults, use [the build guide](../build.md).

The experiment changes the complete image userspace to musl: zbm-rs (both roles),
BusyBox, kmod, ZFS userspace, eudev, their runtime libraries, and test-profile SSH.
The normal glibc outputs remain the comparison baseline. This is not a mixed
image that only changes the Rust executable.

Nixpkgs supports pkgsMusl and pkgsCross.musl64. The native pkgsMusl plan required
511 builds, including rebuilding the native Rust/tool ecosystem. The equivalent
cross package set targets x86_64-unknown-linux-musl using native glibc build tools;
the initial cross plan required 47 builds with cached cross compilers. The chosen path
is pkgsCross.musl64. Host-side Python/uv/cpio/compressors/ukify are not staged.

nix/image.nix accepts runtimePkgs separately from its native builder pkgs.
Kernel and matching ZFS module come from the existing kernelPackages set: kernel
code does not use userspace libc, so rebuilding the kernel would confound the
comparison. The ZFS userspace/module version and ABI assertions remain active.
The module list, loader JSON, compression, Rust optimization and fixture stay the same.

Rust dynamically links musl (-crt-static), sharing the libc already needed by the
C tools. This experiment does not use pkgsStatic or require every tool to be static.
Panic unwinding is retained. The one ELF / two processes architecture is unchanged.

The runtime staging audit records ELF interpreters and DT_NEEDED dependencies,
including static ELF records. A musl image rejects glibc paths, ld-linux
interpreters and libc.so.6 dependencies. The audit report is exported alongside
sizes.json as runtime-audit.json; it is not a runtime application dependency.
A negative host test proves rejection of an ordinary glibc executable.

```sh
nix build .#zbm-rs-efi-musl --out-link target/image-musl
nix build .#zbm-rs-efi-test-musl --out-link target/image-musl-test
uv run --no-project nix/check-runtime-audit.py
cargo xtask smoke --image target/image-musl-test --run target/vm/musl-new --lifecycle
```

## Results (2026-10-06, x86_64)

Both Nix image outputs built successfully. Kernel 6.18.55 and ZFS 2.4.4 stay
unchanged; the kernel SHA256 is identical to the lean glibc baseline. Both
profiles use zstd-19 and Rust opt-level z, FatLTO, one codegen unit.

| Image | Lean glibc EFI | Whole-musl EFI | Reduction |
| --- | ---: | ---: | ---: |
| Production | 24,410,112 bytes (23.28 MiB) | 23,497,728 bytes (22.41 MiB) | 912,384 bytes (3.74%) |
| Test/SSH | 25,980,928 bytes (24.78 MiB) | 25,132,032 bytes (23.97 MiB) | 848,896 bytes (3.27%) |

Production compressed initramfs shrinks from 10,393,175 to 9,480,925 bytes.
The ELF audit passed for both images (39 ELF files in the test profile): no
glibc, libsystemd or SQLite was staged. Rust needs libgcc_s.so.1 and libc.so,
with ld-musl-x86_64.so.1 as its interpreter; it is dynamically linked.

The full UEFI/QEMU TCG smoke/lifecycle run in target/vm/musl-001 passed:
module loading, ZFS create/snapshot/clone/rename/promote/export/discovery,
local-key native encryption and key unload/load, TUI screenshots and keyboard,
manager abort/panic/SIGSEGV, bounded crash recovery, console restoration,
clean restart, signal forwarding, orphan reaping, IPC FD inheritance, emergency
shell, failed-kexec classification, and normal poweroff. It ended with guest
Power down and exit status zero. Actual booting of another OS via kexec is
still unimplemented. Physical hardware and aarch64 musl remain untested.

Host fmt, workspace tests, clippy, JSON configuration checks and the negative
wrong-libc staging test passed. The musl outputs remain optional experimental
profiles; the normal default is still lean glibc. The measured saving is modest
and includes package integration trimming alongside changing libc.

The initial package graph failed on SQLite 3.53.3 auxiliary Tcl/sanitizer test
builds. SQLite itself is not needed by the boot environment and is not present
in the glibc baseline initramfs. The musl package set now disables lastlog2 in
util-linux and NFS client-tracking daemons in nfs-utils, and removes their SQLite
build input. libblkid/libuuid/libmount and exportfs remain available. There is no
SQLite test override or disabled SQLite test suite in the final experiment.

The musl overlay also disables util-linux's systemd and PAM integrations, uses
eudev for libudev consumers (including LVM), and supplies eudev's udevadm to ZFS
vdev_id. These upstream defaults previously pulled systemd-minimal and libsystemd
into the target build graph despite the image using eudev. The revised
build plan contains neither target systemd nor SQLite. Native image assembly
still uses ukify; this host tool is not included in the initramfs.

The pinned Nixpkgs nfs-utils 3.1.1 musl-includes patch has stale context: upstream
inserted limits.h before the blank line where the patch adds libgen.h. The overlay
replaces only that patch with refreshed context, preserving its actual include
fix and all other musl patches. The replacement applies with --fuzz=0.

After patching, nfs-utils also requires getrpcbynumber, which musl does not
provide. libtirpc 1.3.7 defaults its RPC database implementation off. The musl
overlay enables it with --enable-rpcdb, rather than overriding the configure
probe or pretending that libc provides the function.

Two further nfs-utils adjustments keep SQLite out: disable nfsdcld/nfsdcltrack
and omit the fsidd program from support/reexport/Makefile.in. There is no upstream
configure switch for fsidd; libreexport remains built for exportfs. The musl
thread-format patch also covers three new gssd cleanup log calls in 3.1.1,
casting pthread_t to uintptr_t and printing with PRIxPTR. Compiler format checks
remain enabled.
