# The target owns SRK, NvPCR initialization and PCR11 phases.
# Preserve its standard TPM services and vendor definitions in this fixture.
{ pkgs, nixpkgs, system, kernelPackages }:
import ./boot-fixture.nix {
  inherit pkgs nixpkgs system kernelPackages;
  extraNixosModules = [ ({ lib, ... }: {
    security.lsm = lib.mkForce [ "landlock" "lockdown" "yama" "loadpin"
      "safesetid" "apparmor" "ima" "bpf" ];
  }) ];
}
