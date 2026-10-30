# Experimental TPM-authenticated hibernation backport

This is a source port of Matthew Garrett's 17-patch **RFC**,
[Make hibernation work with lockdown, 8 October 2026](https://www.mail-archive.com/linux-efi@vger.kernel.org/msg13028.html),
to Linux **7.2.9**. It keeps the native kernel swap-image backend. It does not
use Sean Rhodes's userspace encrypted-snapshot series.

## Status

As of 30 October 2026, the full Linux 7.2.9 kernel and matching ZFS 2.4.4 build
with patches 0001–0027 passed. The build uses a disposable public VM certificate and retains
forced integrity Lockdown, kexec signatures and module signatures. VM runtime
validation remains in progress; compilation is not a working-hibernation or
security acceptance claim. The port is not installed on the physical host. No
upstream merge or release date is established by the RFC.

The CRB/SHA-256-only and TIS/SHA-1+SHA-256 EFI/kexec probes passed: the NV index
remained protected, PCR5 survived handoff, and target kexec loads were prohibited.
TIS and CRB previously exposed stale EFI INITRD-table writes corrupting the
incoming initramfs. Patch 0026 adopts the allocation fix
[proposed upstream](https://lore.kernel.org/linux-efi/20261012133712.618913-1-sebastien@wenske.fr/)
and passed its full kernel/ZFS rebuild and both handoff probes.

With patches 0001–0026, the full RAM test saved a signed image but rejected restore
before authentication because the firmware E820 map changed across boots. A
two-boot diagnostic located the moving three-page ACPI reclaim range exactly
at the EFI memory-map address in `boot_params`. The TPM/PCR5/INITRD table pool
remained at the same address. Patch 0027 avoids publishing the unused boot-memory
configuration table on x86: the map is already passed and reserved through
`boot_params`. It preserves configuration-table delivery on other architectures
and does not relax the hibernation E820 check. All 27 patches apply cleanly and
match the audited source tree; the dedicated compilation preflight passed.
The complete 27-patch kernel/ZFS rebuild passed. The CRB/SHA-256-only probe then
passed both exact firmware-E820 equality across two cold boots and the real kexec
handoff with unchanged PCR5 and protected NV state. Authenticated RAM retesting
is in progress.

The existing, unpatched 7.2.9 kernel was tested in a disposable VM: forced
integrity Lockdown prevents hibernation and the resume guard leaves the swap
image intact. The separate LUKS2/ZFS native-resume VM test passed with an
ordinary kernel; it does not validate this new TPM signature implementation.

Still required for this port:

- EFI/TPM provisioning and an actual signed RAM-image save/restore in a VM.
- Verification across the loader's kexec transition, including EFI TPM state.
- Rejection of tampered images and unavailable/untrusted TPM provenance before
  any ZFS import or filesystem mount.
- Integration with the loader's enforced-security resume policy, which still
  rejects resume rather than assuming LUKS authenticates a RAM image.
- Physical hardware acceptance after the VM checks pass.

## Patch order and provenance

`0001` through `0007` are upstream TPM prerequisites needed by the RFC's header
and buffer APIs. Their original commit identifiers are:

| Patch | Upstream commit |
| --- | --- |
| 0001 | `e06f28d32f31206c0be32a08b0273e26cfc1fc92` |
| 0002 | `7a5ec92813bc794ab3674aab993fe8f8b5cee9f0` |
| 0003 | `88c5a62c3af7ca6f36dc6ffc52bcfb29cd2990cb` |
| 0004 | `0b9551c189518544b65000a234277298e931c8f3` |
| 0005 | `065cfba1f4545e5948a2ddc5888a27c7641d5bfc` |
| 0006 | `3d9e043dab0a038a53a43570f60bbf4b1e27d63f` |
| 0007 | `55ba91d4c54ab31ee8387bae40e799de467f8269` |

`0008` through `0024` contain the RFC in its original order. Each patch retains
its original author, commit message and upstream link; prerequisite contexts
are adapted to 7.2.9. `0025` preserves the existing `TPM_MAX_RNG_DATA` response
bound and the TPM 1.2 capability types (`cap_t` and their structures/enums) after
the TPM2 header split. The initial adaptation had removed these still-used
definitions. Both fixes retain the original 7.2.9 types and limits.

`0026` keeps the EFI initrd configuration table in ACPI reclaim memory rather
than reclaimable loader data. Otherwise the incoming kexec kernel can zero an
inherited table address already reused by the new initramfs. This is a boot
compatibility fix; it does not weaken signature enforcement or Lockdown.

`0027` is the x86 boot-memory-map fix described above. Unlike the small INITRD
table allocation, the final memory-map buffer occupied a separate, moving ACPI
range in the failing VM. No CRC bypass, fixed-address workaround or relaxed
memory-map option is used.

The corrected series passes the dedicated TPM/EFI/power compilation preflight,
including the EFI stub, TPM kernel-key implementation and RAM-signature backend
(124 objects with integrity Lockdown and forced signatures enabled). The full
kernel/module build and matching ZFS build passed through patch 0026; the full
0027 build is pending. Runtime validation is a separate gate. Patch inputs are
individually content-addressed, so changes elsewhere in the flake do not rebuild
an unchanged protected kernel or invalidate its signed modules.

## Opt-in build

`nix/security-kernel.nix` accepts `hibernationTpmSignature = true`. It defaults to
`false`, rejects other kernel versions when enabled, and adds this patch list
and the required TPM/signature configuration. It retains forced integrity
Lockdown, mandatory module signatures and mandatory kexec signatures. The
insecure TPM hibernation test option is explicitly disabled.

The option only builds a kernel; it does not enable host hibernation, change
the swap configuration, provision a physical TPM, or authorize resume in
zbm-rs. Use public trust certificates as build inputs; private signing keys
must remain outside Git and the Nix store.

An optional early compile check can be constructed by importing
`nix/check-tpm-hibernation-compile.nix` with `{ kernel = kernelPackages.kernel; }`.
It uses the real configuration/toolchain and compiles only TPM, EFI and power,
without treating the default `bzImage` build flag as an additional target.

`lib.mkTpmHibernationProbe` accepts `kernelPackages`, a public `signedKernel`
and `signedModules` produced by the runtime signing tools. It boots through
OVMF with swtpm and forced integrity Lockdown, checks kernel NV-index protection,
then uses `kexec_file_load` and compares PCR5 across the handoff. The same kernel
enters the target role with the early kexec-load prohibition. This optional probe
does not claim firmware Secure Boot or signed RAM-image restore acceptance;
those require the subsequent integration tests. Never pass signing keys to it.

`lib.mkTpmHibernationTest` accepts the same public inputs and exercises the real
ZFS/LUKS2 target resume gate. It must preserve the boot ID and RAM-only state,
then reject a second image with a deliberately corrupted ECDSA signature before
ZFS import. It uses `hibernate=nocompress` so the negative case tests the signature
rather than a decompression checksum. The ordinary encrypted-hibernation check
retains its default configuration and compressed image path.
Both factories accept `tpmDeviceModel = "tpm-tis"` or `"tpm-crb"`. The small
EFI/kexec probe defaults to TIS; the full RAM-restore fixture defaults to CRB,
matching the Framework's current TPM interface. Each interface needs its own
runtime result; selecting it does not establish acceptance.
`tpmBanks` selects `[ "sha1" "sha256" ]` by default or `[ "sha256" ]`, matching
the Framework's read-only observed allocation. Provisioning explicitly disables
other swtpm banks, because PCR_Allocate preserves banks omitted from the request;
the tests assert the actual kernel-exposed allocation, not just the requested
setting. Allocation was checked across a disposable TPM power cycle.

The RAM fixture also accepts `negativeCase = "signature"` (default),
`"tpm-reset"`, or `"tpm-unavailable"`. The reset case clears only the disposable
guest TPM. The unavailable case uses a fixture-only appended cpio flag to unbind
the guest TPM driver before resume; after rejection it restores the exact
original initramfs and retries the still-intact image. This deliberately tests
kernel mechanics without IMA, and is not an authorized enforced-loader handoff.
Consumed-image failures must remain protected across the following power cycle
through the boot-record v2 attempt flag. These new negative variants have not
yet passed runtime acceptance.
`negativeCase = "cold-unavailable"` separately checks a verified empty swap:
repair cold-boot and paging must work with no TPM, while hibernation remains
unavailable and integrity Lockdown remains active.

## Security assumptions

The RFC authenticates RAM images using TPM-generated keys and audited key
provenance. Image confidentiality still needs a separate mechanism such as
LUKS swap. Its EFI/PCR5 and TPM hierarchy assumptions must be checked on each
boot path. Kernel initialization can provision the RFC's reserved TPM NV index
`0x014c4853`; experimentation therefore belongs in disposable VMs first.

Image authentication does not establish that a ZFS pool has remained unchanged
while the system was asleep. The existing resume-before-import ordering and
explicit saved-session cancellation remain necessary.

### kexec is an unresolved acceptance condition

The target EFI stub does not execute during `kexec_file_load`. On x86, the
kexec boot parameters can carry the first kernel's EFI configuration tables;
the RFC's PCR5 table is allocated in ACPI-reclaim memory. This makes inheritance
from a patched loader kernel plausible, but does not establish that the table
survives our exact boot path or that the trust argument remains valid.

Both the loader and hibernating target need the kernel-owned NV-index
protection. A kernel signature alone does not establish that capability.
The agreed Framework deployment uses one shared patched `kernelPackages` and
removes old system generations after the new path passes acceptance. A separate
target kernel build without kexec is not required. Intentional transitions
through old kernels and investigation of their possible TPM-provenance bypass
are outside this deployment's acceptance scope. This does not establish that
arbitrary privileged kexec chains preserve the RFC's trust assumptions.
The target command line additionally uses
`sysctl.kernel.kexec_load_disabled=1`, applied before initramfs userspace. Loader
and rescue omit it, retaining the ability to hand off to the target.

Acceptance covers the actual EFI-to-patched-loader-to-patched-target path,
TPM evidence/NV protection across that handoff, authenticated RAM restoration,
and rejection of tampered images or invalid/unavailable TPM evidence. Do not
bypass the PCR5 check or remove enforced-mode resume rejection to make a VM
test pass. See the full [agreed architecture](../../../docs/hibernation-design.md#agreed-protected-boot-architecture-not-yet-implemented).
