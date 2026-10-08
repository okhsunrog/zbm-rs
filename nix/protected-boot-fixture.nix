# Target TPM scope is ordinary SRK operation after a v262 loader-owned NvPCR.
# The target's v261 vendor NvPCR defaults must not reinitialize those indices.
{ pkgs, nixpkgs, system, kernelPackages }:
import ./boot-fixture.nix {
  inherit pkgs nixpkgs system kernelPackages;
  extraNixosModules = [ ({ lib, ... }: {
    security.lsm = lib.mkForce [ "landlock" "lockdown" "yama" "loadpin"
      "safesetid" "apparmor" "ima" "bpf" ];
    environment.etc = lib.genAttrs [ "nvpcr/hardware.nvpcr" "nvpcr/login.nvpcr"
      "nvpcr/cryptsetup.nvpcr" "nvpcr/verity.nvpcr" ] (_: { text = ""; });
  }) ];
}
