# Explicit Framework 13 Core Ultra / Insyde profile. Owner signing is external.
{ pkgs, source, moduleSource, hardwareManifest
, kernelTrustedCertificates, targetAuthorities, imaCertificate, hostId ? null }:
import ./image.nix {
  inherit pkgs source hardwareManifest kernelTrustedCertificates targetAuthorities imaCertificate hostId;
  kernelPackages = pkgs.linuxPackages_latest;
  kernelPolicy = "configure";
  profile = "host-only";
  extraModulePackages = kernelPackages: [
    (import ./insyde-efivarfs.nix {
      inherit pkgs kernelPackages;
      sourceDirectory = moduleSource;
    })
  ];
  forcedModules = [ "insyde_efivarfs" ];
  moduleOptions.insyde_efivarfs = "expose=1";
  loaderConfig = {
    ui = { timeout_secs = 0; show_snapshots = true; };
    zfs.import_policy = "host-id";
    kernel_args = [];
    security = {
      mode = "enforce";
      require_firmware_secure_boot = true;
      tpm.policy = "optional";
    };
  };
}
