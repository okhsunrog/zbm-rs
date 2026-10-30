# Linux 7.2.9: seven TPM prerequisites, the 17-patch RFC, and one port fix.
# Experimental and opt-in; see patches/hibernation-tpm-7.2.9/README.md.
[
  { name = "0001-tpm-initial-step-to-reorganize-tpm-public-headers"; patch = ./patches/hibernation-tpm-7.2.9/0001-tpm-initial-step-to-reorganize-tpm-public-headers.patch; }
  { name = "0002-tpm-move-tpm2-specific-definitions-to-the-command-header"; patch = ./patches/hibernation-tpm-7.2.9/0002-tpm-move-tpm2-specific-definitions-to-the-command-header.patch; }
  { name = "0003-tpm-move-tpm-common-base-definitions-to-the-command-header"; patch = ./patches/hibernation-tpm-7.2.9/0003-tpm-move-tpm-common-base-definitions-to-the-command-header.patch; }
  { name = "0004-tpm-buf-merge-tpm-buf-boundary-error-and-tpm-buf-overflow"; patch = ./patches/hibernation-tpm-7.2.9/0004-tpm-buf-merge-tpm-buf-boundary-error-and-tpm-buf-overflow.patch; }
  { name = "0005-tpm-buf-remove-chip-parameter-from-tpm-buf-append-handle"; patch = ./patches/hibernation-tpm-7.2.9/0005-tpm-buf-remove-chip-parameter-from-tpm-buf-append-handle.patch; }
  { name = "0006-tpm-buf-memory-safe-allocations"; patch = ./patches/hibernation-tpm-7.2.9/0006-tpm-buf-memory-safe-allocations.patch; }
  { name = "0007-tpm-buf-add-tpm-buffer-support-header-for-standalone-reuse"; patch = ./patches/hibernation-tpm-7.2.9/0007-tpm-buf-add-tpm-buffer-support-header-for-standalone-reuse.patch; }
  { name = "0008-tpm-define-a-kernel-owned-tpm-nv-index-that-can-t-be-modified-by-userland"; patch = ./patches/hibernation-tpm-7.2.9/0008-tpm-define-a-kernel-owned-tpm-nv-index-that-can-t-be-modified-by-userland.patch; }
  { name = "0009-efi-add-a-mechanism-to-modify-tpm-state-depending-on-kernel-security-features"; patch = ./patches/hibernation-tpm-7.2.9/0009-efi-add-a-mechanism-to-modify-tpm-state-depending-on-kernel-security-features.patch; }
  { name = "0010-tpm-allow-tpm2-start-auth-session-to-start-an-audit-session"; patch = ./patches/hibernation-tpm-7.2.9/0010-tpm-allow-tpm2-start-auth-session-to-start-an-audit-session.patch; }
  { name = "0011-tpm-log-commands-executed-in-an-audit-session"; patch = ./patches/hibernation-tpm-7.2.9/0011-tpm-log-commands-executed-in-an-audit-session.patch; }
  { name = "0012-tpm-add-a-kernel-attestation-key-and-signed-audit-digest-retrieval"; patch = ./patches/hibernation-tpm-7.2.9/0012-tpm-add-a-kernel-attestation-key-and-signed-audit-digest-retrieval.patch; }
  { name = "0013-tpm-use-tpm2-nv-readpublic-to-read-nv-index-names"; patch = ./patches/hibernation-tpm-7.2.9/0013-tpm-use-tpm2-nv-readpublic-to-read-nv-index-names.patch; }
  { name = "0014-tpm-add-in-kernel-support-for-reading-nv-indices"; patch = ./patches/hibernation-tpm-7.2.9/0014-tpm-add-in-kernel-support-for-reading-nv-indices.patch; }
  { name = "0015-tpm-add-nv-define-undefine-and-write-helpers"; patch = ./patches/hibernation-tpm-7.2.9/0015-tpm-add-nv-define-undefine-and-write-helpers.patch; }
  { name = "0016-tpm-provision-the-kernel-nv-index-at-registration"; patch = ./patches/hibernation-tpm-7.2.9/0016-tpm-provision-the-kernel-nv-index-at-registration.patch; }
  { name = "0017-tpm-move-the-bounds-checked-response-reader-to-a-header"; patch = ./patches/hibernation-tpm-7.2.9/0017-tpm-move-the-bounds-checked-response-reader-to-a-header.patch; }
  { name = "0018-tpm-add-kernel-signing-key-creation-with-audited-provenance"; patch = ./patches/hibernation-tpm-7.2.9/0018-tpm-add-kernel-signing-key-creation-with-audited-provenance.patch; }
  { name = "0019-tpm-add-signing-with-the-kernel-signing-key"; patch = ./patches/hibernation-tpm-7.2.9/0019-tpm-add-signing-with-the-kernel-signing-key.patch; }
  { name = "0020-tpm-add-verification-of-kernel-signing-key-provenance"; patch = ./patches/hibernation-tpm-7.2.9/0020-tpm-add-verification-of-kernel-signing-key-provenance.patch; }
  { name = "0021-pm-hibernate-add-image-digest-and-signature-page-infrastructure"; patch = ./patches/hibernation-tpm-7.2.9/0021-pm-hibernate-add-image-digest-and-signature-page-infrastructure.patch; }
  { name = "0022-pm-hibernate-sign-and-verify-images-with-a-kernel-generated-tpm-key"; patch = ./patches/hibernation-tpm-7.2.9/0022-pm-hibernate-sign-and-verify-images-with-a-kernel-generated-tpm-key.patch; }
  { name = "0023-pm-hibernate-refuse-to-verify-images-with-a-virtual-tpm"; patch = ./patches/hibernation-tpm-7.2.9/0023-pm-hibernate-refuse-to-verify-images-with-a-virtual-tpm.patch; }
  { name = "0024-pm-hibernate-allow-hibernation-under-lockdown-with-signed-images"; patch = ./patches/hibernation-tpm-7.2.9/0024-pm-hibernate-allow-hibernation-under-lockdown-with-signed-images.patch; }
  { name = "0025-tpm-preserve-7.2-header-definitions"; patch = ./patches/hibernation-tpm-7.2.9/0025-tpm-preserve-7.2-header-definitions.patch; }
]
