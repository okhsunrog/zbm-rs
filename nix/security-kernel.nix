# A separate loader kernel; owner private keys are never derivation inputs.
{ pkgs, kernelPackages, trustedCertificates }:
let
  lib = pkgs.lib;
  bundle = pkgs.runCommand "zbm-loader-trusted-certificates" {
    nativeBuildInputs = [ pkgs.openssl ];
  } (lib.concatMapStringsSep "\n" (certificate: ''
    openssl x509 -in ${lib.escapeShellArg "${certificate}"} -out certificate.pem
    cat certificate.pem >> $out
  '') trustedCertificates);
  kernel = kernelPackages.kernel.override {
    structuredExtraConfig = lib.mapAttrs (_: value: lib.mkForce value) (with lib.kernel; {
      EFI = yes; EFI_STUB = yes; EFIVAR_FS = yes;
      KEXEC = yes; KEXEC_FILE = yes; KEXEC_SIG = yes;
      KEXEC_SIG_FORCE = yes; KEXEC_BZIMAGE_VERIFY_SIG = yes;
      MODULE_SIG = yes; MODULE_SIG_FORCE = yes; MODULE_SIG_ALL = no;
      # Only the explicit owner certificates authorize modules. Do not generate
      # an extra signing key which might escape through a development output.
      MODULE_SIG_KEY = freeform "";
      SYSTEM_TRUSTED_KEYRING = yes;
      SYSTEM_BLACKLIST_KEYRING = yes;
      SYSTEM_TRUSTED_KEYS = freeform "${bundle}";
      SIGNED_PE_FILE_VERIFICATION = yes;
      SECONDARY_TRUSTED_KEYRING = yes;
      INTEGRITY = yes; INTEGRITY_SIGNATURE = yes;
      INTEGRITY_ASYMMETRIC_KEYS = yes; INTEGRITY_TRUSTED_KEYRING = yes;
      INTEGRITY_PLATFORM_KEYRING = yes;
      SECURITY_LOCKDOWN_LSM = yes; SECURITY_LOCKDOWN_LSM_EARLY = yes;
      LOCK_DOWN_KERNEL_FORCE_INTEGRITY = yes;
      IMA = yes; IMA_APPRAISE = yes; IMA_READ_POLICY = yes;
      IMA_WRITE_POLICY = yes; IMA_APPRAISE_BOOTPARAM = no;
      IMA_ARCH_POLICY = yes; IMA_APPRAISE_BUILD_POLICY = yes;
      IMA_APPRAISE_REQUIRE_POLICY_SIGS = yes;
      IMA_KEYRINGS_PERMIT_SIGNED_BY_BUILTIN_OR_SECONDARY = yes;
      IMA_DEFAULT_HASH_SHA256 = yes;
      LSM = freeform "landlock,lockdown,yama,loadpin,safesetid,apparmor,ima,bpf";
      TMPFS_XATTR = yes;
    });
  };
in
assert lib.assertMsg (trustedCertificates != []) "Enforced loader kernel needs public trust certificates";
pkgs.linuxPackagesFor kernel
