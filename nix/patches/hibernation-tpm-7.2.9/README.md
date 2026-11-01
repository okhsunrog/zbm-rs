# Experimental TPM-authenticated hibernation backport

This is a source port of Matthew Garrett's 17-patch **RFC**,
[Make hibernation work with lockdown, 8 October 2026](https://www.mail-archive.com/linux-efi@vger.kernel.org/msg13028.html),
to Linux **7.2.9**. It keeps the native kernel swap-image backend. It does not
use Sean Rhodes's userspace encrypted-snapshot series.

## Status

As of 1 November 2026, the full Linux 7.2.9 kernel and matching ZFS 2.4.4 build
with patches 0001–0028 passed. The VM build uses a disposable public certificate and retains
forced integrity Lockdown, kexec signatures and module signatures. Authenticated
RAM restoration, subsequent hibernation and the complete enforced Secure Boot
chain passed in disposable VMs. The owner's signed build is also installed on a
physical Framework Laptop 13, where repeated authenticated hibernation/resume
cycles passed with Secure Boot enabled. See the
[physical validation results](../../../docs/verification.md#physical-framework-validation-2026-11-01).
No upstream merge or release date is established by the RFC.

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
handoff with unchanged PCR5 and protected NV state. One authenticated RAM restore
then passed, retaining the boot ID, RAM-only data, healthy ZFS and the target
kexec prohibition. The second hibernation failed because restored RAM retained
a stale NULL-primary context from the previous TPM boot. Patch 0028 addresses
that repeat-cycle defect; its targeted compilation and full kernel/ZFS build
passed. The CRB/SHA-256 probe also passed exact E820 and TPM handoff-address
equality across firmware boots, PCR5/NV protection and the real kexec transition.
With patch 0028, CRB/SHA-256 and TIS/dual-bank VMs restored the original kernel
boot ID and RAM-only state, retained healthy ZFS and created the next signed
hibernation image. The complete OVMF Secure Boot → enforced zbm-rs → PE/IMA/CMS
authorized NixOS chain also restored RAM and created the next image. Its UI test
verified F10 preserving the entire swap disk, cancellation confirmation/Esc,
discard-only staying in the menu and protected cold boot after cancellation.
Missing capabilities and a mismatched resume policy were refused on cold boots.

Separate negative-only runs on the 27-patch kernel rejected a modified ECDSA
signature and a cleared guest TPM before ZFS import. After Linux consumed the
image signature, boot-record v2 still required explicit cancellation on the
following boot. These checks deliberately skip the initial restore to isolate
failure handling; they do not establish repeat-cycle acceptance.

The existing, unpatched 7.2.9 kernel was tested in a disposable VM: forced
integrity Lockdown prevents hibernation and the resume guard leaves the swap
image intact. The separate LUKS2/ZFS native-resume VM test passed with an
ordinary kernel; it does not validate this new TPM signature implementation.

Patch 0028 also rejected modified signature trailers, a changed RAM-payload byte
with its original signature trailer intact, and a cleared guest TPM before pool
import. The following boot still required explicit cancellation after a consumed
image failed verification. These tests retain an initial successful restoration;
they are not the older negative-only diagnostic runs.

Other deployments still require matching public trust inputs and their own
hardware validation. LUKS unlock never substitutes for RAM authentication.

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

`0028` is a local authenticated-context extension, not part of Garrett's RFC.
It transfers the current NULL-primary context through reserved EFI ACPI memory
after full RAM-image verification. The signed image binds the original NULL
name and exact handoff address. A fresh nonce challenge to the original
provenance AK authenticates the audit transcript for NULL CreatePrimary before
the handoff becomes ready. TPM resume checks the old identity, consumes the
context once and wipes it; abandoned handoffs are cleared. It does not disable
in-session TPM reset detection. The format is `LNXHPV02` and the build contract
requires backend `linux-tpm-signature-v2`, context-handoff protocol 1. See
[the trust boundary](../../../docs/hibernation-design.md#kernel-roles-and-authenticated-memory).

The corrected series passes the dedicated TPM/EFI/power compilation preflight,
including the EFI stub, TPM kernel-key implementation and RAM-signature backend
(124 objects with integrity Lockdown and forced signatures enabled). The full
kernel/module build and matching ZFS build passed through patch 0028. All 7409
modules in the public VM artifact were signed and independently verified, as
was its PE kernel signature.
Runtime validation is a separate gate. Patch inputs are
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

The RAM fixture also accepts `negativeCase = "signature"` (default), `"payload"`,
`"tpm-reset"`, or `"tpm-unavailable"`. The payload case changes a copied RAM byte
without altering its valid signature trailer. The reset case clears only the disposable
guest TPM. The unavailable case uses a fixture-only appended cpio flag to unbind
the guest TPM driver before resume; after rejection it restores the exact
original initramfs and retries the still-intact image. This deliberately tests
kernel mechanics without IMA, and is not an authorized enforced-loader handoff.
Consumed-image failures must remain protected across the following power cycle
through the boot-record v2 attempt flag. Signature, payload and reset runs passed
with patch 0028 after successful RAM restoration and the next hibernation.
The CRB/SHA-256 unavailable-TPM run passed three restorations of the same original
RAM session: TPM absence stopped resume before pool import without consuming the
image, the exact clean initramfs/TPM allowed retry, and the next hibernation and
third restoration succeeded. The finish hook cleared the durable attempt state.
The same three-cycle/TPM-absence scenario passed with `compressedImage = true`;
the separate complete protected-chain runtime also passed all seven checks with
`ZBM_RESUME_VM_COMPRESSED=1`. Both assert actual compression/decompression logs.
The default uncompressed tamper tests remain separate so a decompression CRC
cannot stand in for image-signature rejection.
`negativeCase = "cold-unavailable"` separately checks a verified empty swap:
repair cold-boot and paging must work with no TPM, while hibernation remains
unavailable and integrity Lockdown remains active. This run passed. The explicit repair variant
uses `systemd.tpm2_wait=0` to prevent a later firmware/device synchronization wait
after the deliberately unbound TPM; native image authentication is unchanged.

## Security assumptions

The RFC authenticates RAM images using TPM-generated keys and audited key
provenance. Image confidentiality still needs a separate mechanism such as
LUKS swap. Its EFI/PCR5 and TPM hierarchy assumptions must be checked on each
boot path. Kernel initialization can provision the RFC's reserved TPM NV index
`0x014c4853`; experimentation therefore belongs in disposable VMs first.

Image authentication does not establish that a ZFS pool has remained unchanged
while the system was asleep. The existing resume-before-import ordering and
explicit saved-session cancellation remain necessary.

### Supported kexec path and remaining acceptance

The target EFI stub does not execute during `kexec_file_load`. On x86, the
kexec boot parameters can carry the first kernel's EFI configuration tables;
the RFC's PCR5 table is allocated in ACPI-reclaim memory. This makes inheritance
from a patched loader kernel possible. Disposable CRB and TIS probes verified
the supported handoff, PCR5 inheritance and NV protection. The separate complete
signed loader chain and authenticated RAM restoration also passed; these are
disposable VM results and do not establish physical laptop acceptance.

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
bypass the PCR5 check or treat test-only policy enablement as deployment
acceptance. See the full [agreed architecture](../../../docs/hibernation-design.md#agreed-protected-boot-architecture).
