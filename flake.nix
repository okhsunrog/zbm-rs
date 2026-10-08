{
  description = "zbm-rs: one PID-1/manager executable and canonical ZFS EFI images";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };
  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; config.allowUnfree = true; };
      mkImage = args: import ./nix/image.nix ({ inherit pkgs; source = self; } // args);
      muslPkgs = import ./nix/musl-userspace.nix { inherit pkgs; };
      leanUserspace = import ./nix/lean-userspace.nix { inherit pkgs; };
      optimizedRust = { optLevel = "z"; lto = "fat"; codegenUnits = 1; };
      production = mkImage { };
      testing = mkImage { testProfile = true; };
    in {
      lib.mkImage = mkImage;
      lib.mkProtectedBootFixture = args: import ./nix/protected-boot-fixture.nix
        ({ inherit pkgs nixpkgs system; } // args);
      packages.${system} = {
        default = production;
        zbm-rs-efi = production;
        zbm-rs-efi-test = testing;
        zbm-rs-efi-test-snapshot = mkImage { testProfile = true;
          loaderConfig = builtins.fromJSON (builtins.readFile ./config/test-snapshot.json); };
        zbm-rs = production.passthru.binary;
        zbm-rs-boot-fixture = import ./nix/boot-fixture.nix { inherit pkgs nixpkgs system; };
        # One libc across the complete staged userspace; native tools build the
        # image, and the kernel/ZFS module pair stays on the same kernelPackages.
        zbm-rs-efi-musl = mkImage { runtimePkgs = muslPkgs; };
        zbm-rs-efi-test-musl = mkImage { runtimePkgs = muslPkgs; testProfile = true; };
        zbm-rs-efi-full-userspace = mkImage { zfsUserspace = pkgs.zfs_2_4.override { enablePython = false; }; udevPackage = pkgs.systemdMinimal; };
        zbm-rs-efi-test-full-userspace = mkImage { testProfile = true; zfsUserspace = pkgs.zfs_2_4.override { enablePython = false; }; udevPackage = pkgs.systemdMinimal; };
        zbm-rs-efi-test-zstd = mkImage { testProfile = true; initramfsCompression = "zstd"; rustProfile = optimizedRust; zfsUserspace = pkgs.zfs_2_4.override { enablePython = false; }; udevPackage = pkgs.systemdMinimal; };
        zbm-rs-efi-test-standalone-udev = mkImage { testProfile = true; initramfsCompression = "zstd"; rustProfile = optimizedRust; zfsUserspace = pkgs.zfs_2_4.override { enablePython = false; }; udevPackage = leanUserspace.systemdUdev; };
        # Backwards-compatible names; lean is now the normal image.
        zbm-rs-efi-test-lean = testing;
        zbm-rs-efi-lean = production;
        zbm-rs-efi-host-only-example = mkImage { profile = "host-only"; hardwareManifest = ./nix/hardware-example.json; };
        zbm-rs-efi-test-host-only-scsi = mkImage { testProfile = true; profile = "host-only"; hardwareManifest = ./nix/scsi-hardware-example.json; };
      };
      checks.${system} = {
        hardware-probe = pkgs.runCommand "zbm-hardware-probe-check" { nativeBuildInputs = [ pkgs.uv pkgs.python3 ]; } ''
          export UV_CACHE_DIR="$TMPDIR/uv-cache"
          uv run --offline --no-project --python ${pkgs.python3}/bin/python ${self}/nix/check-hardware-probe.py
          touch $out
        '';
        image-size = production.passthru.sizeCheck;
        module-config = import ./nix/check-module-config.nix {
        inherit self nixpkgs pkgs system;
        binary = production.passthru.binary;
        };
      };
      nixosModules.default = import ./nix/nixos-module.nix { inherit self; };
      nixosModules.snapshotBoot = import ./nix/snapshot-boot.nix;
      devShells.${system}.default = pkgs.mkShell { packages = with pkgs; [ cargo rustc rustfmt clippy qemu cpio uv python3 gzip xz zstd patchelf ]; };
    };
}
