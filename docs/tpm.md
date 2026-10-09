# TPM and measured boot

zbm-rs measures a verified prepared target without owning the selected OS's TPM
setup. Secure Boot/IMA/BootAuthorization authenticate boot inputs; TPM measurements
record which inputs were prepared. These are separate controls.

## Boot sequence and ownership

```text
UEFI -> systemd-stub -> zbm-rs kernel/initramfs -> kexec -> OS kernel/initramfs -> OS
          PCR11          read TPM2/PCR15        PCRs survive   OS phases/setup
                         verify boot inputs
                         record plan in PCR15
```

| Operation | Owner | Current behavior |
| --- | --- | --- |
| Authenticate the loader UKI | UEFI Secure Boot | Firmware verifies its EFI signature when Secure Boot is enabled. |
| Measure the loader UKI | systemd-stub | Measures UKI resources in PCR11 when firmware provides TPM measurement support. |
| Probe TPM availability | zbm-rs PID1 | Checks the resource-manager device and TPM2 version, then reads SHA-256 PCR15 through sysfs. |
| Authenticate a selected target | Loader kernel and root broker | Enforced path checks kernel/module signatures, initramfs IMA signature and pinned BootAuthorization, including arguments/root selection. |
| Record a prepared target | Root broker | After successful verified `kexec_file_load`, extends PCR15 with the exact prepared-plan record's SHA-256. |
| Prepare SRK and NvPCRs | Selected OS | Its native initramfs/userspace services choose the setup API, indices, priorities and policy. |
| Extend OS phases in PCR11 | Selected OS | Its initramfs/userspace controls `enter-initrd`, `leave-initrd` and later phases. |

The loader does not allocate NV indices, create a persistent SRK, measure hardware
identity into NvPCRs or extend OS phase events in PCR11. Limited NV capacity and
OS-specific NvPCR setup failures are handled by the OS's own policy. TPM-dependent
ZFS unlock, remote attestation and rollback resistance are not implemented.

## Loader policy

TPM behavior is immutable image configuration, separate from `security.mode` and
`requireFirmwareSecureBoot`. Add this to an existing configured enforced profile:

```nix
programs.zbm-rs.settings.security.tpm.policy = "optional";
```

The corresponding JSON field is `security.tpm.policy`. Its default is `off`.

| Value | Startup | Verified target preparation |
| --- | --- | --- |
| `off` | Skips the loader's TPM readiness probe. | Skips the loader's PCR15 event. |
| `optional` | Records `ready`, `unavailable` or `degraded` evidence. | Attempts the PCR15 extend; failure is diagnostic and does not weaken boot-input checks. |
| `required` | Refuses startup if TPM2/SHA-256 PCR15 cannot be read. | Refuses preparation if the PCR15 extend fails. |

`required` does not require an SRK or any allocated NvPCR. `off` does not disable
firmware/systemd-stub measurements or target-OS TPM services. TPM policy does not
enable verified boot: the current selected-target event is wired to the enforced
verified executor; the ordinary `security.mode=off` path performs no target-plan
measurement even when its startup TPM probe is enabled.

See [configuration](configuration.md#secure-boot-configuration) for the complete
image/trust inputs. TPM-enabled images currently require the glibc userspace
profile. The runtime measurement path uses the ordinary systemd
`systemd-pcrextend` helper and its libraries; there is no separate v262 setup provider or systemd service manager
underneath PID1.

## Evidence and its meaning

| Loader file | Meaning |
| --- | --- |
| `/run/zbm-rs/security.json` | Firmware state, image policy, IMA/lockdown readiness and startup TPM evidence. |
| `/run/zbm-rs/tpm.json` | Enabled-probe state, the initial SHA-256 PCR15 value and probe errors. |
| `/run/zbm-rs/tpm-target.json` | Exact verified artifact evidence and final ordered arguments for the latest prepared attempt. |
| `/run/zbm-rs/tpm-target.log` | Bounded measurement helper output. |
| `/run/zbm-rs/tpm-target-error` | Diagnostic from an unsuccessful prepared-target measurement. |
| `/run/log/systemd/tpm2-measure.log` | Userspace measurement events produced by the helper. |

The event is `zbm-rs:target-prepared:v1:<sha256-of-record>`. It includes the actual
selected root/clone through the final arguments. Native input loading succeeds
before this event is recorded; kexec execution happens afterward. Preparation
can still be followed by cleanup, cancellation or another attempt, so the event
does not prove successful OS execution. The runtime plan file describes the latest attempt;
PCR15 accumulates extends and is not reset between manager restarts.

Startup readiness is a read-only capability observation, not proof of firmware
measurement or an attestation. Kernel/IMA/BootAuthorization checks remain mandatory
in an enforced image even when TPM is missing or optional measurement fails.

## What crosses kexec

PCR values and NV indices reside in the TPM and survive kexec. Files in the loader's
`/run` do not automatically become files in the selected OS's new initramfs.
The target EFI stub is not executed by `kexec_file_load`, and its synthetic
`/.extra` resources are not supplied automatically.

No loader-owned NvPCR initialization metadata needs transport: the loader creates
none. The OS initializes its own indices and carries its own runtime state from
initramfs into userspace using its normal boot flow.

The loader's PCR15 event log and prepared-plan records still need a defined
transport into the OS for later replay/attestation. That transport is pending.
The current harness captures and replays them before handoff. The loader does not
rewrite a signed target initramfs at boot to append mutable logs. IMA measurement
log preservation across kexec is a separate kernel mechanism; IMA appraisal of
an initramfs does not itself establish log transport.

## OS-owned signed PCR policies

An OS using the v262 signed-policy NvPCR API needs a public policy key and signatures
packaged in its own trusted initramfs. Its owner authorizes the actual loader UKI
measurement plus the OS's early PCR11 phase sequence. The signatures permit TPM
operations and do not replace kernel/initramfs/argument verification.

When the loader UKI changes, update the OS policy to authorize the new measurement.
If a previous loader or snapshot remains a supported recovery target, its
initramfs must contain policy signatures for the loader states it is expected to
accept. The loader publisher signs boot artifacts/IMA policy; it does not currently
produce these OS-owned PCR policies or embed `.pcrpkey`/`.pcrsig`.

The former loader fields `security.tpm.nvpcrs`, `security.tpm.required_nvpcrs`,
Nix `settings.security.tpm.nvpcrs`, `settings.security.tpm.requiredNvpcrs` and
`image.pcrPublicKey` are removed.
Old configurations fail explicitly and must be updated/rebuilt. The publisher
rejects obsolete images containing `pcr-public.pem`.

Implementation is in [the runtime TPM module](../crates/tui/src/tpm.rs),
[the verified executor](../crates/tui/src/manager/executor.rs) and
[the NixOS module](../nix/nixos-module.nix).

## Verified scope and remaining work

The persistent OVMF/swtpm harness confirms PCR11 replay from the UKI without loader
phase events, exact prepared-plan PCR15 replay, empty loader NV/persistent handle
lists, required missing-TPM refusal and optional absence without weaker boot checks.
Real NixOS ZFS-root and trusted snapshot-clone boot also pass with native target
TPM services and unmasked NvPCR definitions. Those target journals show a new SRK
created in initramfs and four NvPCRs initialized later by native systemd v261.

This establishes compatibility with that native v261 setup. Target v262 signed
NvPCR policy/consumer integration, PCR15 log transport and physical Framework
acceptance remain separate gates.

See [current acceptance results](verification.md#coverage),
[reproduction workflow](security-testing.md) and
[remaining milestones](roadmap.md#secure-boot-and-tpm-milestone).
