{
  description = "zbm-rs: one PID-1/manager executable and canonical ZFS EFI images";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    zfskit = { url = "github:okhsunrog/zfskit/6389c1be98b7721eec9858cc4916cb866d09d5c2"; flake = false; };
  };
  outputs = { self, nixpkgs, zfskit }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; config.allowUnfree = true; };
      mkImage = args: import ./nix/image.nix ({ inherit pkgs; source = self; zfsSource = zfskit; } // args);
      leanUserspace = import ./nix/lean-userspace.nix { inherit pkgs; };
      optimizedRust = { optLevel = "z"; lto = "fat"; codegenUnits = 1; };
      production = mkImage { };
      testing = mkImage { testProfile = true; };
    in {
      lib.mkImage = mkImage;
      packages.${system} = {
        default = production;
        zbm-rs-efi = production;
        zbm-rs-efi-test = testing;
        zbm-rs = production.passthru.binary;
        zbm-rs-efi-full-userspace = mkImage { zfsUserspace = pkgs.zfs_2_4.override { enablePython = false; }; udevPackage = pkgs.systemdMinimal; };
        zbm-rs-efi-test-full-userspace = mkImage { testProfile = true; zfsUserspace = pkgs.zfs_2_4.override { enablePython = false; }; udevPackage = pkgs.systemdMinimal; };
        zbm-rs-efi-test-zstd = mkImage { testProfile = true; initramfsCompression = "zstd"; rustProfile = optimizedRust; zfsUserspace = pkgs.zfs_2_4.override { enablePython = false; }; udevPackage = pkgs.systemdMinimal; };
        zbm-rs-efi-test-standalone-udev = mkImage { testProfile = true; initramfsCompression = "zstd"; rustProfile = optimizedRust; zfsUserspace = pkgs.zfs_2_4.override { enablePython = false; }; udevPackage = leanUserspace.systemdUdev; };
        # Backwards-compatible names; lean is now the normal image.
        zbm-rs-efi-test-lean = testing;
        zbm-rs-efi-lean = production;
        zbm-rs-efi-host-only-example = mkImage { profile = "host-only"; hardwareManifest = ./nix/hardware-example.json; };
      };
      checks.${system}.image-size = production.passthru.sizeCheck;
      nixosModules.default = import ./nix/nixos-module.nix { inherit self; };
      devShells.${system}.default = pkgs.mkShell { packages = with pkgs; [ cargo rustc rustfmt clippy qemu cpio uv python3 gzip xz zstd ]; };
    };
}
