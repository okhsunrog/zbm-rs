# Experimental TPM-authenticated hibernation backport

This is a source port of Matthew Garrett's 17-patch **RFC**,
[Make hibernation work with lockdown, 8 October 2026](https://www.mail-archive.com/linux-efi@vger.kernel.org/msg13028.html),
to Linux **7.2.9**. It keeps the native kernel swap-image backend. It does not
use Sean Rhodes's userspace encrypted-snapshot series.

## Status

As of 30 October 2026, compilation and VM validation of this port are still in
progress. This checkpoint is not a working-hibernation or security acceptance
claim. The port is not installed on the physical host. No upstream merge or
release date is established by the RFC.

The existing, unpatched 7.2.9 kernel was tested in a disposable VM: forced
integrity Lockdown prevents hibernation and the resume guard leaves the swap
image intact. The separate LUKS2/ZFS native-resume VM test passed with an
ordinary kernel; it does not validate this new TPM signature implementation.

Still required for this port:

- A successful kernel and matching ZFS build.
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
are adapted to 7.2.9. `0025` is a local port fix preserving the existing
`TPM_MAX_RNG_DATA` response bound after moving TPM declarations to public
headers. The first compilation attempt exposed the missing definition.

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
