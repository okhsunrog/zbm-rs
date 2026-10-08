# Secure Boot and verified boot design

This is the accepted design, not a claim of completed Secure Boot acceptance. It is
intended to make verified boot a first-class zbm-rs feature for both generic Linux
and NixOS, while keeping the existing Linux/initramfs architecture. Configuration
names below include implemented API; see [configuration](configuration.md). Implementation
work and deferred features are tracked in [the roadmap](roadmap.md), and acceptance
requirements in [verification](verification.md#planned-secure-boot-acceptance).

## Current implementation

Implementation has started. The shared schema provides `off`/`enforce`, a separate
firmware requirement, public authority paths and explicit TPM capabilities. The image
mode is also compiled into the ELF; runtime JSON cannot downgrade an enforced
binary. PID 1 initializes restricted IMA policy before launching the manager and
uses bounded protected recovery on every enforced failure path. Root shell,
administrative rollback and persistent clone/promotion are unavailable in that mode.

The root broker is a fresh role of the same ELF, in the manager's supervised
process group. Before Tokio/UI startup the manager opens diagnostic descriptors,
starts the broker, clears groups/capabilities, changes all IDs to 65534 and sets
`NO_NEW_PRIVS`. The broker independently resolves pool GUIDs and boot candidates,
ignores caller paths/import policy/extra arguments, and owns kexec. Its private
stream uses bounded length-prefixed typed JSON; malformed or timed-out exchanges
invalidate the connection. UI telemetry is not an authorization record.

Core now verifies detached binary CMS signatures against explicit X.509 pins.
It ignores embedded signer certificates and has no implicit CA store. Authorization
binds sizes/SHA-256, exact ordered arguments, one typed ZFS root slot, source
dataset and snapshot-clone permission. Read-only OS roots carry
`/boot/zbm-rs/authorizations/<artifact-id>.json` and the matching `.cms`; the ID is
SHA-256 of `kernel-sha256:initramfs-sha256` in lowercase hexadecimal. The signed
JSON also carries the detached `security.ima` signature for the initramfs.
Authorization precedes persistent clone creation and is repeated against the
actual owned clone before handoff. An opaque plan holds copied/sealed, read-only
memfd inputs and exact arguments. Source changes cannot alter those sealed bytes.

The canonical Nix artifact remains an unsigned UKI. The separate owner publisher
signs the IMA policy and modules, independently verifies their signatures, and
signs a fresh final UKI. A configured Linux 6.18.55 loader with matching ZFS passed
signed OVMF acceptance, including actual IMA appraisal of sealed read-only memfds
and a file-based handoff to a signed synthetic second kernel. Unsigned kernels,
bad IMA signatures, altered CMS/content/arguments and legacy kexec are rejected.
This proves the tested mechanism, not installed-OS, snapshot or physical acceptance.
TPM startup and target-prepared measurements are implemented with a separate v262
provider. Initial swtpm acceptance covers successful setup/PCR replay, missing or
invalid signed policy, required missing TPM and optional degradation/unavailability;
scarcity, stale-index and interrupted-setup cases remain pending. All published default images
remain `off`; do not install this work as a verified production loader yet.

The executor uses `kexec_file_load` exclusively and does not silently fall back to
`kexec_load`. It reports kernel verification/lockdown errors, unloads a prepared
kernel on failure, unmounts its OS roots and exports only pools matched to import
ownership records. `KexecStarting` distinguishes failed handoff from an ordinary
manager crash. These safeguards alone do not authenticate Bootspec, initramfs or
the command line; the new enforced broker supplies that authorization. Existing
unsigned OVMF evidence remains separate from the signed synthetic-input scenario.

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
accepts a kernel package set. The `image.kernelPackages` override keeps
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

## Hardware lessons and TPM integration

The Framework deployment exposed failures that the image pipeline and harness
must reproduce instead of relying on unsigned boot success:

- A kernel requiring signed IMA policy rejects raw rule text written to securityfs.
  The loader carries a policy and detached signature, restores `security.ima` on
  the policy inode, then writes its absolute pathname to the policy interface.
  It reads back the required `KEXEC_INITRAMFS_CHECK` appraisal rule. A certificate
  file in the initramfs alone does not establish trusted `.ima` keyring membership.
- Do not assume the distribution kernel includes forced kexec/module signatures,
  forced lockdown, writable/readable IMA policy or the `ima` LSM. Inspect the final
  `.config` and runtime enforcement. The pinned default Nix kernel lacks several
  required controls; use validation or a separately configured loader variant.
- Authentication attempts and BE discovery are separate state transitions. A
  loaded ZFS key is not owner authorization for a shell. Failed unlock must remain
  retryable without stale discovery state or endless password/error loops. Unlock
  UI is still unimplemented; its tests belong to the recovery milestone.
- Insyde firmware can lose EFI variable enumeration while known-variable reads
  still work. An empty listing is not Secure Boot disabled. A compatibility
  provider must be explicit, DMI-scoped, signed for the selected loader kernel,
  and run before firmware evidence or TPM/EFI-dependent setup. Never change keys,
  clear the TPM or relax verification automatically after an enumeration failure.
- TPM NV space is limited. Ordinary TPM2/SRK operations can work while one NvPCR
  allocation fails. Select only needed NvPCRs and allocate in declared priority
  order; do not reserve every hardware/login/cryptsetup/verity index by default.
  Required capabilities block handoff on failure; optional failures are reported
  without weakening target authorization or reallocating unrelated owner indices.

TPM support is now an active implementation requirement. It includes ordinary
TPM2 capability evidence, measured loader/target transitions and optional native
NvPCR integration with an owner-signed PCR policy. It does not unlock ZFS. Normal
PCRs and NvPCRs have separate purposes; a corporate TPM consumer must not be
assumed to need NvPCRs merely because it uses TPM2. The profile selects actual
capabilities rather than exposing a single switch promising everything.

Keep distinct public/private key roles for firmware enrollment/EFI signing,
target-kernel and module signatures, IMA files/policy, BootAuthorization and PCR
policy authorization. A signed PCR policy approves measured states for a TPM
operation; it neither signs boot artifacts nor overrides failed verification.
Owner keys stay outside Nix. Public keys and signed policies may be packaged;
production key material is never reused in disposable TPM/OVMF fixtures.

The loader UKI runs through systemd-stub once. `kexec_file_load` does not run the
target EFI stub again or automatically transfer its `.pcrsig`, `.pcrpkey` and
`.extra` resources. PCR11 loader-image/phase measurements alone do not identify
the selected OS kernel/initramfs/arguments. Kernel-side kexec/IMA measurements,
explicit selected-target events and target initialization need a defined event
log/handoff contract. Phase ordering across the two initrds must match signed
policy; never assume a policy for one `enter-initrd` applies after a loader phase
transition and another initrd. A snapshot root must be represented by the actual
final plan without treating an unrestricted dataset name as authenticated state.

Before completing the profile, test PCR replay and signed-policy authorization
with swtpm under real OVMF measured boot, including firmware off/unknown, no TPM,
full NV space, stale indices and interrupted setup. The exact target-event PCR,
log transport and target-initrd policy integration remain implementation work;
there is no remote attestation or rollback-resistance claim from diagnostic logs.

The implemented provider is systemd v262, built separately from host systemd.
The earlier v261 anchor-secret NvPCR API is deliberately not a fallback. Startup
extends `enter-initrd` in PCR11, then initializes selected NvPCRs with an
owner-signed policy using `policyref=initrd`. Only afterward does it measure the
SMBIOS product identity into a selected hardware NvPCR. Priorities allocate
hardware, cryptsetup, login, then verity. Initializing login/cryptsetup/verity
does not imply that those installed-OS consumers are integrated. Required
capabilities stop startup on failure; optional failures produce degraded evidence.
No helper clears the TPM or requests automatic deletion of foreign indices.

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
- Authenticated automatic ZFS unlock and rollback-resistant state. TPM measured
  boot and NvPCR support are active work above; measurements/PCR logs alone are
  not attestation. Reading encrypted boot inputs still requires explicit normal
  passphrase unlock, which must not authorize administrative recovery.
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
