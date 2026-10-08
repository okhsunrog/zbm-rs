# Owner signing and protected VM acceptance

Nix constructs an unsigned image from public trust inputs. Owner deployment signs
an output copy outside `/nix/store`; it never supplies private keys to a Nix
derivation. Default flake images still use `off`. An `enforce` image must be
constructed with `lib.mkImage` or the documented NixOS options and matching public
trust. Neither this workflow nor a VM report enrolls keys on the physical host.

## Publisher

An enforced Nix result includes `root.cpio`, `kernel.config`,
`kernel-capabilities.json`, `ima-policy`, `stub.efi` and `os-release` alongside
the ordinary image files. `nix/publish-loader.py` unpacks only bounded newc archives
with safe paths and inode types. It checks the packaged IMA certificate against
the owner signing certificate, signs the policy, signs and verifies each staged
module, repacks the root and builds a fresh UKI. EFI signing is the last step.
The ready output directory is renamed into place only after all steps succeed.
Signing, size-limit or verification failures leave no ready output directory.

```sh
uv run --no-project python nix/publish-loader.py result-enforce deployment-new \
  --efi-key /private/efi.key --efi-cert /public/efi.pem \
  --ima-key /private/ima.key --ima-cert /public/ima.pem \
  --module-key /private/module.key --module-cert /public/module.pem \
  --sign-file /matching-kernel-dev/lib/modules/VERSION/build/scripts/sign-file \
  --evmctl /path/to/evmctl --ukify /path/to/ukify
```

The module signer and target-kernel signer must be trusted by the loader kernel.
The IMA certificate must be a **non-CA leaf** with digitalSignature usage, signed
by a certificate accepted by the loader's restricted IMA keyring. Merely copying
an arbitrary public certificate into the image does not establish that trust.
The tested kernel refuses to add a CA certificate to `.ima`.

`evmctl --sigfile` also attempts to set an xattr. Unprivileged owner staging uses
`--xattr-user --sigfile`; the publisher independently verifies the detached
signature. Trusted early startup restores it as `security.ima` on the policy
inode, then writes its **absolute pathname** to securityfs. Writing raw policy
rules is rejected when the kernel requires signed policies.

Production publishing refuses the test/SSH profile. Disposable tests must
explicitly pass `--allow-fixture`; `--fixture-directory` additionally inserts
synthetic target inputs. Such an image is never a production recovery image.
Private fixture signing keys remain outside the guest and Git.

## TPM ownership

The loader publisher signs boot artifacts and IMA policy. It does not sign or
embed an NvPCR initialization policy and does not require a PCR signing key.
Images built with the former loader-owned `image.pcrPublicKey` option must be
rebuilt; the publisher rejects obsolete images containing `pcr-public.pem`.

TPM-enabled loader startup reads SHA-256 PCR15 through sysfs. It creates no
persistent SRK/NV indices and leaves PCR11 at the systemd-stub UKI measurement.
Only the verified target-prepared event extends PCR15, using the ordinary Nix
systemd `systemd-pcrextend` helper. The selected OS owns SRK/NvPCR initialization
and all PCR11 OS phase events. If it uses signed-policy NvPCRs, its owner must
package the policy/public key in its own initramfs and authorize the actual loader
UKI plus the OS's phase sequence; the target EFI stub is not rerun by kexec.
No production private keys enter Nix or either initramfs.

## Persistent scenario

`xtask/fixtures/security.py` constructs a synthetic second kernel/initramfs and
signed authorizations from explicitly supplied disposable key paths. It creates
valid inputs plus wrong IMA signature, unsigned kernel, wrong CMS, wrong arguments
and changed-initramfs cases. Run its `--help` for inputs. The target only proves
actual kernel handoff, final arguments, a changed boot ID and guest poweroff;
it is not a complete installed Linux/ZFS root.

The OVMF variables template must contain only disposable firmware trust matching
the fixture EFI signer. Use matching 4M Secure Boot CODE/VARS firmware. Each run
copies the template rather than modifying it. A production enrollment or host
TPM must never be used for this scenario.

```sh
cargo xtask secure-smoke --image deployment-test-new \
  --secure-vars /disposable/OVMF_VARS.fd --run target/vm/verified-new --tcg

# The harness owns and reaps both QEMU and a fresh disposable swtpm process:
cargo xtask secure-smoke --image deployment-tpm-test-new \
  --secure-vars /disposable/OVMF_VARS.fd --run target/vm/verified-tpm-new \
  --swtpm --tcg
```

The scenario checks actual firmware Secure Boot, signed IMA readiness, native
kernel/initramfs acceptance and rejection, exact CMS/argument/content checks,
legacy-kexec refusal, zero UI capabilities and IDs 65534, shell refusal, protected
crash recovery and a real second-kernel handoff. The optional swtpm path checks
read-only TPM readiness and the target-prepared PCR15 event. Startup and prepared
measurement logs are retained; the prepared event is not an execution proof.
Guest TPM state lives in the fresh sibling `<run>.tpm` directory.

TPM-enabled scenarios capture the real firmware event log, systemd userspace
event log, exact prepared-plan JSON and SHA-256 PCR11/PCR15 values. The independent
`xtask/fixtures/check_tpm.py` replays the logs against those actual PCRs and checks
the prepared event's digest against the exact final plan. No loader PCR11 phase
event is permitted, including after manager crash/restart. Read-only `tpm2_getcap`
queries on the disposable guest explicitly use `/dev/tpmrm0` and require empty
NV/persistent handle lists before loading and after protected recovery. The
setup helper, NvPCR definitions and initialization files must be absent. The VM
test transport waits for NIC/SSH readiness separately from visible recovery or
manager readiness.

Use `secure-smoke --expect-tpm rejected` with a required-profile image and no
`--swtpm` to verify missing-device refusal. With an optional-profile image and no
TPM, use `--expect-tpm unavailable`: every boot-input rejection and actual verified
handoff check must still pass. Optional read/extend failures remain diagnostics;
they never select weaker boot verification. Bad/missing NvPCR signature tests
belong to OS policy integration, not loader readiness.

## Installed NixOS and snapshot scenarios

`xtask/fixtures/authorize_nixos.py` publishes a fresh disposable copy of a generated
NixOS root archive. It rejects traversal, devices, hardlinks and symlink ancestors,
never follows guest absolute symlinks on the host, signs/verifies all target module
files, signs the kernel and final initramfs, and uses `nix/authorize-boot.py` for
each ordered plan. Keys are passed only to signing tools; the original Nix store
is never rewritten. This helper packages test roots, not an installed-system updater.

Authorize the exact NixOS systemd-root plan, with no global snapshot-test argument:
the target proof identifies ordinary or snapshot-clone root from the actual ZFS
mount. Each generation has its own argument ID beneath the shared artifact ID.
The indexed root value stays variable while all other tokens and their order are
fixed. JSON/CMS signatures still enforce source dataset and owned-clone permission.

```sh
cargo xtask boot-smoke --image deployment-protected-loader \
  --fixture deployment-authorized-nixos --secure-vars /disposable/OVMF_VARS.fd \
  --swtpm --run target/vm/verified-nixos-new

cargo xtask boot-smoke --snapshot --image deployment-protected-loader \
  --fixture deployment-authorized-nixos --secure-vars /disposable/OVMF_VARS.fd \
  --swtpm --run target/vm/verified-snapshot-new
```

These extend the existing harness, including QMP keyboard/PNG observations,
SSH provisioning on disposable disks, manager-restart/ownership checks and the
selected OS's multi-user proof. `boot-smoke` owns/reaps QEMU and swtpm, using the
same shared swtpm lifetime helper as `secure-smoke`. The ordinary protected case
adds a syntactically valid generation with unauthorized parameters and requires
the broker to refuse it with an empty kexec slot before explicitly selecting the
trusted older generation. The snapshot case exercises authorized clone preparation,
foreign-owner refusal, explicit discard, restart reconciliation and actual clone boot.

The ordinary target TPM gate requires a ready SRK service and persisted public SRK
after kexec, not just loader readiness. These test roots include `tpm_crb` in their
preboot module list and keep the target's native NvPCR definitions unmasked. The
loader creates no SRK or NvPCR policy that could conflict with target setup.
Target-specific signed NvPCR policy/consumer acceptance and PCR15 log handoff
remain separate gates.

The target proof captures early/late TPM setup journals when a device is
available, then requires `ZBM_TARGET_TPM_SRK_READY` before publishing boot success.

`lib.mkProtectedBootFixture { kernelPackages = configuredLoaderKernelPackages; }`
is the canonical Nix profile retaining required LSMs and the target's ordinary TPM
configuration. Supply the same configured kernel/ZFS set as the protected loader.
Build the unsigned archive,
then publish a fresh outside-store copy with `authorize_nixos.py`:

```sh
uv run --no-project python xtask/fixtures/authorize_nixos.py \
  result-protected-fixture deployment-authorized-nixos \
  --kernel-key /private/kernel.key --kernel-cert /public/kernel.pem \
  --module-key /private/module.key --module-cert /public/module.pem \
  --ima-key /private/ima.key --ima-cert /public/ima.pem \
  --authority-key /private/authorization.key --authority-cert /public/authorization.pem \
  --sign-file /matching-kernel-dev/lib/modules/VERSION/build/scripts/sign-file \
  --evmctl /path/to/evmctl
```

## Recorded scope and remaining gates

For installed-OS acceptance, `nix/boot-fixture.nix` accepts a separate
`kernelPackages` and `extraNixosModules`. The default fixture still uses the stock
kernel; the protected profile is explicitly selected.
The protected fixture must use the tested configured kernel with matching ZFS,
and explicitly retain `ima` and `lockdown` in NixOS `security.lsm`: the ordinary
NixOS default otherwise emits a narrower `lsm=` argument despite the kernel's
configured default. Fixture preparation is not target capability approval.

`target/security-fixtures/nixos-signed-001` contains two authorized NixOS
generations sharing one image pair but distinct argument indexes. Owner publishing
signed 7297 installed-root modules, the preboot modules, kernel and final initramfs
outside Nix. `verified-nixos-002` passes real ZFS-root boot through the broker/menu;
`verified-nixos-snapshot-001` passes writable trusted clone boot with source and
snapshot preserved. Their actual selection/prepared-clone PNGs were inspected.
Before removing loader-owned TPM setup, the SRK-scoped target fixture additionally
passed ordinary and snapshot boot in `verified-nixos-srk-001` / `verified-nixos-snapshot-srk-001`, including
early/late target SRK reuse and public-key persistence without extra NvPCR allocation.

`target/vm/verified-004/security-report.json` passed under OVMF Secure Boot using
Linux 6.18.55 and matching ZFS. This confirmed IMA appraisal of sealed, read-only
memfd inputs and actual file-based kexec. The earlier `verified-001` preserved
the CA-certificate rejection; `verified-003` preserved a screenshot stability
failure caused by a blinking cursor. The harness now pauses QEMU while capturing
stable screenshots and resumes only if it was running.

The current OS-owned TPM matrix and earlier experiments are recorded in
[verification](verification.md). The new ordinary and snapshot runs show the
selected OS creating its own SRK and initializing its native v261 NvPCRs without
target masks. Signed-policy v262 consumer acceptance remains a separate gate.
Before production: verify generic-Linux authorization, encrypted-root passphrase
UI, expected embedded kernel
certificates, algorithm/profile restrictions, PCR15 log transport across kexec,
OS-owned signed NvPCR policy/consumer integration and physical Framework recovery. Insyde
EFI enumeration recovery is a separate DMI-scoped provider and remains pending
here. No attestation, rollback-resistance or whole-root-integrity promise follows
from this synthetic scenario.
