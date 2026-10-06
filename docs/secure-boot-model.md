# Secure Boot and verified boot design

This is the accepted design, not a claim that Secure Boot is implemented. It is
intended to make verified boot a first-class zbm-rs feature for both generic Linux
and NixOS, while keeping the existing Linux/initramfs architecture. Configuration
names below are planned API; see [configuration](configuration.md). Implementation
work and deferred features are tracked in [the roadmap](roadmap.md), and acceptance
requirements in [verification](verification.md#planned-secure-boot-acceptance).

## Current implementation

The canonical Nix artifact is an unsigned UKI. There is no signing service, key
management, BootAuthorization verifier, IMA appraisal setup or protected recovery
mode. The current privileged manager owns ZFS operations and kernel handoff; PID 1
can start an unrestricted emergency shell. These are development capabilities,
not the accepted boundary for an enforced image.

The executor uses `kexec_file_load` exclusively and does not silently fall back to
`kexec_load`. It reports kernel verification/lockdown errors, unloads a prepared
kernel on failure, unmounts its OS roots and exports only pools matched to import
ownership records. `KexecStarting` distinguishes failed handoff from an ordinary
manager crash. These safeguards do not authenticate Bootspec, initramfs or the
command line. Current OVMF tests run without Secure Boot enforcement.

Initial NixOS support uses `org.nixos.bootspec.v1`. Specialisations and extra
initrd extensions remain outside the initial path; `initrdSecrets` execution is
rejected. Installed-OS scripts must not run inside the trusted loader to prepare
an initramfs. Linux and NixOS argument parsing have different current limits;
parsing an argument successfully is not authorization to boot it.

## Security promise and limits

An enforced loader boots only owner-authorized kernel and initramfs bytes with
an authorized command line and root-selection rule. Failed or unavailable checks
stop that boot attempt, rather than selecting a weaker loader or a root shell.
With firmware Secure Boot enabled and the expected firmware trust configuration,
this extends firmware trust through the loader to those target boot inputs.

This does not authenticate every file in the target root filesystem, prove that
signed code is safe, protect against a compromised signing authority, or guarantee
rollback prevention. ZFS encryption protects confidentiality; unlocked data and
boot metadata still need authorization. Dataset names, GUIDs, encryption-key
availability and Nix store path hashes are not proof of the contents' authenticity.
The installed OS remains responsible for its own runtime integrity policy.

The initial design does not require two separate main/development images and does
not add a native UEFI ZFS implementation. One ELF remains the process entry point.

## Image policy and firmware state

The image's immutable configuration selects `security.mode`:

| Mode | Behavior |
| --- | --- |
| `off` | Existing development behavior; no verified-boot claim. |
| `enforce` | Target authorization, kernel signature checking, initramfs appraisal and command-line restrictions are mandatory. No automatic bypass or unrestricted recovery shell. |

`security.require_firmware_secure_boot` is an independent boolean, initially
false by default. When true, an enforced image also refuses OS handoff unless
firmware Secure Boot is confirmed enabled. When false, `enforce` still verifies
all target inputs if firmware Secure Boot is disabled; the loader itself then
lacks firmware-established authenticity. Unknown firmware state never weakens
checks; it fails the additional requirement when that requirement is enabled.

These are build-time image configuration values, not Cargo features. Turning off
Secure Boot in firmware does not turn off `enforce`. There is no automatic
verified/development profile switch, and no independently configurable skip for
initramfs, command-line verification or legacy kexec. Missing/invalid production
configuration must not silently select `off` or a developer default.

A separate development image can be a packaging convenience later. Signing a
weakened image with a production-trusted EFI key would provide a bypass; it must
not become an implicitly trusted recovery entry.

## Chain of trust

```text
Firmware PK / KEK / db / dbx policy
  | authenticates EFI signature of loader UKI
  v
Loader UKI: stub + loader kernel + initramfs + config + public trust roots
  | trusted early setup: module trust, lockdown, IMA policy and certificates
  v
Privileged broker resolves an untrusted candidate
  | verifies signed BootAuthorization and exact kernel/initramfs bytes
  | constructs and authorizes final command line and root selection
  v
Prepared immutable boot inputs
  | kexec_file_load: loader kernel verifies target kernel signature
  | IMA appraisal: loader kernel verifies target initramfs signature
  | broker: checks final command line against BootAuthorization
  v
Selected target kernel + initramfs + authorized command line
  | target OS applies its own root-filesystem/runtime policy
  v
Selected dataset / authorized snapshot clone
```

The EFI signature covers the loader UKI's embedded resources. The embedded loader
command line must be authoritative under Secure Boot; systemd-stub ignores
invocation-time replacement arguments when Secure Boot is active and a command
line is embedded. External UKI add-ons and companion resources must be explicitly
reviewed and constrained; signing the base UKI does not authorize arbitrary
external resources. Firmware variables are observed by trusted early startup;
the manager cannot report a weaker firmware state to the broker.

The three target checks are complementary: a kernel signature alone does not bind
an initramfs or command line; an initramfs signature alone does not bind a kernel;
a BootAuthorization binds the whole permitted combination. The kernel does not
understand zbm-rs's custom command-line policy, so the broker enforces it.
The initial ZFS target path requires an initramfs. An initramfs-free target would
need a separate explicit supported authorization rule, not a missing-file bypass.

Initial integration must establish one tested firmware trust path. Direct firmware
`db` enrollment is the intended baseline; shim/MOK integration is not assumed to
exist. Firmware acceptance of the loader and the loader kernel's keyrings are
separate: enrollment alone must not be assumed to populate every required keyring.

## Keys and ownership

| Role | Verifies | Public trust location |
| --- | --- | --- |
| Loader EFI signing | Complete loader UKI | Firmware trust database (or a separately supported verified shim path) |
| Boot authorization | Permitted target artifacts and selection/argument policy | Immutable loader trust store |
| Target kernel signing | PE signature accepted by `kexec_file_load` | Appropriate loader-kernel trusted keyring |
| Module signing | Loader kernel modules, including ZFS; target modules use target-kernel trust | Respective kernel's trusted keyrings |
| IMA signing | Target initramfs bytes | Loader IMA keyring with an authenticated certificate loading path |
| Owner recovery authorization | Future privileged administration | Separate future design; not just a pool-unlock result |

Roles are explicit even if a personal installation reuses a key. Trust authorities
may have scopes; a module key is not automatically a boot-policy authority.
Production private keys never enter Git, the Nix store, derivation inputs,
initramfs, UKI or VM fixture outputs. Build and deployment exchange public
certificates, unsigned artifacts and verification reports. Deployment signs an
output copy; it does not mutate an immutable Nix-store result.

CI may generate a temporary module-signing key, sign the kernel's modules and ZFS,
embed its public certificate in that kernel and discard the private key. This
requires verifying that neither logs nor artifacts/cache retain the private key.
A fresh random signing key makes those outputs vary between builds; deterministic
archive construction is not a promise of bit-identical randomly signed artifacts.
Long-lived owner EFI/authorization/IMA keys belong to the separate signing step.
Key rotation needs an overlap/recovery plan; revocation must cover all applicable
roles rather than merely deleting a certificate from one new image.

## Signed BootAuthorization

Linux discovery, NixOS Bootspec and ZFS properties produce candidates, not trusted
boot plans. Both OS adapters use a common signed authorization model that binds:

- Format version, architecture, artifact identity and signing authority/scope.
- Exact kernel and initramfs sizes and SHA-256 digests.
- Final allowed argument structure, including typed root-selection slots.
- Permitted root/snapshot/clone selection rules.
- Required loader capabilities and approved target security profile.

SHA-256 authenticates bytes only as part of a verified signature over the policy.
The encoding and signature backend remain implementation decisions: use a standard
reviewed format/library, not new cryptography. Parsing must be bounded and reject
unsupported versions, duplicate or ambiguous fields and malformed signatures.
A signed claim about a target profile needs a trusted artifact producer; an
untrusted `.config` file beside a kernel cannot establish its actual capabilities.

Arguments are represented as structured tokens and rendered deterministically.
The broker permits only the signed fixed arguments and explicit typed variations.
For example, an approved root slot can select a dataset or broker-owned clone
within the authorized environment while keeping every other token fixed. This
must reject added, repeated or conflicting parameters; a blacklist of known
unsafe parameters is insufficient. `settings.kernelArgs` must also satisfy this
policy; image configuration is not permission to append arbitrary target arguments.

Snapshots keep their original authorized kernel/initramfs bytes. A broker-created
clone can change the root token under the signed selection rule after validating
its origin and operation ownership. Names and GUIDs support routing and cleanup;
they do not cryptographically attest all dataset contents. Old unsigned snapshots
are not silently grandfathered in. A trusted publisher must authorize their boot
inputs before an enforced loader offers them as bootable.

Allowing a previously trusted snapshot is distinct from revoking obsolete code.
A future security epoch/minimum-version policy may restrict targets, but a new
loader's policy cannot prevent firmware from launching an older still-trusted
loader. Strong rollback prevention needs firmware revocation/key rotation or
protected state such as a properly designed TPM policy. It is outside the first
verified-boot promise.

## Privileged boundary and handoff API

The current root manager and public-path `BootPlan` are insufficient security
boundaries. The accepted direction keeps one ELF with distinct runtime roles:
PID 1 supervises an unprivileged UI/manager and a trusted privileged broker. Core
owns authorization models and policy; TUI renders results and submits typed
requests; generic ZFS mechanisms stay in zfskit, not application trust policy.

The manager requests target IDs and bounded operations, not arbitrary commands,
paths or a caller-provided assertion that verification succeeded. The broker
resolves discovery again, applies policy, owns privileged ZFS actions and checks
handoff independently. A private socket establishes peer/lifecycle identity, not
permission to execute any requested action. Pool/clone ownership records,
serialized operations, cancellation/draining and cleanup remain required.

```text
CandidateBootPlan
  -> verify_and_prepare(policy, candidate)
  -> VerifiedBootPlan
  -> LoadedKernel
  -> handoff
```

`VerifiedBootPlan` has private fields, no public unchecked constructor and no
`Deserialize`. It owns finalized arguments, prepared input handles and verification
evidence. A root-capable UI could still invoke kexec directly, so type safety alone
is not sufficient; OS privilege separation must enforce the broker boundary.

The exact bytes verified must be the bytes passed to `kexec_file_load`. Holding an
open file descriptor prevents path substitution but not modification of its inode.
Preparation therefore needs immutable private staging with only the broker able
to write, finalized read-only handles and an enforced no-mutation lifetime. The
implementation must prove this property, including helper-process privileges.
Sealed memfd is a candidate requiring an IMA/kexec compatibility experiment, not an
accepted assumption. Native ZFS fs-verity support is not assumed.

A detached IMA signature can be authenticated and attached to a prepared private
copy when the snapshot/archive lacks `security.ima`. This must preserve the signed
bytes and use actual attr tooling; never change a read-only snapshot or regenerate
its target initramfs during boot. IMA appraisal must check the prepared file used
by kexec. Kernel enforcement rejects legacy `kexec_load` and unsigned modules.
No fallback loader path may circumvent the broker's policy.

## Recovery and failure handling

In `enforce`, failed verification, manager crashes, PID-1 error/unwind recovery,
last-ditch paths and returned kexec attempts must all retain the same restrictions.
The default protected recovery offers diagnostics, manager restart, reboot and
poweroff. It does not spawn an unrestricted root shell before or after unlocking
ZFS. Shell access could replace RAM-backed policy, call kexec directly or alter
boot inputs; a mode-0444 config file does not prevent that.

Future owner-authorized administrative recovery needs its own authentication and
capability design. Merely observing a loaded encryption key, an imported pool or
a familiar GUID is not owner authentication: attacker-controlled metadata can
imitate those signals. Encrypted datasets do not justify opening a shell on an
unrelated loader failure. Boot-environment scripts and unrestricted subprocess
requests must not provide another route to privileged execution.

## Kernel selection and build integration

There are two kernels: the loader's kernel inside its UKI, and the selected OS
kernel loaded by kexec. zbm-rs does not compile kernels at boot and does not
silently rebuild or reconfigure the installed OS kernel.

Nix selects `kernelPackages` and obtains the loader kernel from cache or builds it.
The NixOS module currently inherits `boot.kernelPackages`; `lib.mkImage` already
accepts a kernel package set. The planned `image.kernelPackages` override keeps
loader selection independent from the host. `image.kernelPolicy` has two values:

- `validate` (default): inspect the supplied final kernel and fail the image build
  if the selected security policy's capabilities/certificates are missing.
- `configure`: build a separate loader-kernel variant with the required security
  configuration and matching ZFS modules. Never mutate host `boot.kernelPackages`.

For `enforce`, validate the final architecture-specific kernel configuration and
artifacts, not just requested Nix options. Required capabilities include EFI/EFI
stub, file-based kexec and architecture signature verification, forced kexec
signature checking, module signatures/enforcement, lockdown with early integrity
restrictions, IMA appraisal/certificate loading, appropriate trusted keyrings and
hash algorithms. Exact `CONFIG_*` requirements depend on the supported kernel and
architecture and must be maintained as a validated capability matrix. Kconfig
may reset requested flags because dependencies are missing.

The proposed x86_64 capability checks include `CONFIG_EFI`, `CONFIG_EFI_STUB`,
`CONFIG_KEXEC_FILE`, `CONFIG_KEXEC_SIG`, `CONFIG_KEXEC_SIG_FORCE`, the architecture's
signed bzImage verification support, `CONFIG_MODULE_SIG`, `CONFIG_MODULE_SIG_FORCE`,
`CONFIG_SECURITY_LOCKDOWN_LSM` with early forced integrity restrictions, and IMA
appraisal/X.509 loading support. Validate the effective LSM list, certificate
keyring paths and SHA-256 availability too. This is a capability requirement, not
a reusable unchecked `.config` fragment for every kernel version. The early IMA
policy must appraise the kexec initramfs read hook, enforce rejection rather than
audit/fix mode, and prevent subsequent unauthorized policy weakening. Startup
must reject loader arguments that disable required protections, including when
firmware Secure Boot is off.

Trusted early initramfs setup loads authenticated IMA certificates and installs
appraisal policy before untrusted inputs are handled. Forced kernel controls
cannot be undone by a UI toggle or by disabling firmware Secure Boot. Certificate
files merely copied into the UKI are insufficient: verify the certificates' actual
kernel/keyring integration. Validate every preboot module's signature, including
ZFS, before packing the final loader initramfs, and retain existing ABI and ZFS
userspace/module version checks.

```text
Nix options -> shared Rust configuration validation
  -> select or configure separate loader kernel + matching ZFS
  -> validate final kernel configuration, certificates and module signatures
  -> trusted initramfs setup + immutable JSON/public trust store
  -> unsigned UKI + capability/artifact report
  -> separate owner signing and installation
  -> verify deployed bytes and run signed-boot acceptance
```

Startup must check required enforcement is actually active and refuse handoff on
failure; build-time validation is not runtime evidence. The target OS kernel is
an externally produced, signed and authorized artifact. Its module/DKMS trust and
runtime IMA policy are separate from the loader's appraisal of the target initramfs.
Production artifacts exclude test SSH, fault hooks and test private keys.

## User-visible evidence

Show firmware state (`enabled`, `disabled`, `unknown`), configured loader mode,
authorization result, kernel acceptance, initramfs appraisal and command-line
approval separately. Root-filesystem contents remain unattested by this feature.
A success message must not imply firmware protection when firmware verification
was disabled or imply whole-system integrity from a verified initramfs. Logs and
UI are diagnostic evidence, not remote cryptographic attestation.

## Explicit backlog and open implementation choices

The following are not required for the initial enforced image:

- Runtime UI permission **“Boot without a trusted signature”** (Russian:
  **«Загрузить без доверенной подписи»**). If added later, only confirmed disabled
  firmware Secure Boot can permit it, never enabled/unknown state. Approval is for
  one exact target/argument set and one boot, with reapproval after changes and no
  persistent trust checkbox. Conditional kernel enforcement must be designed
  before irreversible lockdown/IMA setup; the initial forced kernel cannot offer
  such a toggle. There is no automatic permission merely because Secure Boot is off.
- Owner-authenticated administrative shell/recovery and optional development-image
  packaging, without production-signing a bypass.
- TPM measured boot, authenticated automatic ZFS unlock and rollback-resistant
  state. Measurements/PCR logs alone are not attestation. Reading encrypted boot
  inputs requires an explicit unlock/measurement ordering; public authenticated
  metadata or a loader-bound unlock policy are future designs, not implicit support.
- Whole-root integrity policy, target-wide IMA appraisal, and shim/MOK support.

Signature encoding/backend, certificate rotation tooling, the exact broker IPC
and sandbox, immutable staging/IMA interoperability, and per-kernel capability
checks require implementation design and tests. These choices must preserve the
accepted boundaries above; they must not become silent bypass options.

## References

- [Unified Kernel Image specification](https://uapi-group.org/specifications/specs/unified_kernel_image/)
- [systemd-stub manual source](https://github.com/systemd/systemd/blob/main/man/systemd-stub.xml)
- [Linux file-based kexec](https://github.com/torvalds/linux/blob/master/kernel/kexec_file.c)
- [Linux IMA hooks](https://github.com/torvalds/linux/blob/master/security/integrity/ima/ima_main.c)
- [Linux module signature checks](https://github.com/torvalds/linux/blob/master/kernel/module/signing.c)
- [Linux lockdown](https://github.com/torvalds/linux/blob/master/security/lockdown/lockdown.c)
- [NixOS Bootspec RFC](https://github.com/NixOS/rfcs/blob/master/rfcs/0125-bootspec.md)
- [Linux kexec verification selftests](https://github.com/torvalds/linux/blob/master/tools/testing/selftests/kexec/test_kexec_file_load.sh)
