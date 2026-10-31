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
      lib.mkTpmHibernationProbe = args: import ./nix/check-tpm-hibernation-kexec.nix
        ({ inherit pkgs; } // args);
      lib.mkTpmHibernationTest = args: import ./nix/check-encrypted-hibernation.nix
        ({ inherit self pkgs nixpkgs system; } // args);
      lib.mkProtectedBootFixture = args: import ./nix/protected-boot-fixture.nix
        ({ inherit pkgs nixpkgs system; } // args);
      lib.mkProtectedHibernationDriver = args: import ./nix/protected-hibernation-driver.nix
        ({ inherit self pkgs; } // args);
      packages.${system} = {
        default = production;
        zbm-rs-efi = production;
        zbm-rs-efi-test = testing;
        zbm-rs-efi-test-preferred = mkImage { testProfile = true;
          loaderConfig = (builtins.fromJSON (builtins.readFile ./config/test.json)) // {
            nixos.preferred_specialisation = "niri";
          };
        };
        zbm-rs-efi-test-resume = mkImage {
          testProfile = true;
          loaderConfig = {
            resume = {
              device = { type = "luks2"; partuuid = "12345678-1234-1234-1234-123456789abc";
                luks_uuid = "ca55a30a-636e-4e69-9e68-bb36bf380408"; mapping = "resume-swap"; };
              credential = { type = "zfs-unlock"; pool_guid = "42"; encryption_root = "zbm_resume"; };
            };
            boot.mode = "menu";
            zfs.import_policy = "host-id";
          };
        };
        zbm-rs-efi-test-snapshot = mkImage { testProfile = true;
          loaderConfig = builtins.fromJSON (builtins.readFile ./config/test-snapshot.json); };
        zbm-rs = production.passthru.binary;
        zbm-rs-boot-fixture = import ./nix/boot-fixture.nix { inherit pkgs nixpkgs system; };
        zbm-rs-boot-fixture-specialisations = import ./nix/boot-fixture.nix {
          inherit pkgs nixpkgs system;
          withSpecialisation = true;
        };
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
        kernel-patch-isolation = import ./nix/check-kernel-patch-isolation.nix { inherit pkgs; };
        signed-module-tree = import ./nix/check-signed-module-tree.nix { inherit self pkgs; };
        encrypted-hibernation = import ./nix/check-encrypted-hibernation.nix { inherit self pkgs nixpkgs system; };
        encrypted-suspend-then-hibernate = import ./nix/check-encrypted-hibernation.nix {
          inherit self pkgs nixpkgs system; sleepOperation = "suspend-then-hibernate";
        };
        cold-resume-disabled = import ./nix/check-encrypted-hibernation.nix {
          inherit self pkgs nixpkgs system; negativeCase = "cold-disabled";
        };
        luks2-resume-device = import ./nix/check-luks2-resume.nix { inherit self pkgs; };
        prune-nixos = import ./nix/check-prune-nixos.nix { inherit pkgs; };
        resume-config = import ./nix/check-resume-config.nix { inherit self nixpkgs pkgs system; };
        efi-install = import ./nix/check-efi-install.nix { inherit pkgs self; };
        config-binary-reuse = import ./nix/check-config-reuse.nix { inherit pkgs mkImage; };
        native-nixos = import ./nix/check-native-nixos.nix { inherit self pkgs nixpkgs system; };
        module-options = let appliance = mkImage {
          forcedModules = [ "nvme" ];
          moduleOptions.zfs = "zfs_arc_max=67108864";
          hostId = "1234abcd";
        }; in pkgs.runCommand "zbm-required-module-options-check" {
          nativeBuildInputs = [ pkgs.cpio ];
        } ''
          cpio --quiet -i --to-stdout etc/modprobe.d/zbm-rs.conf < ${appliance.runtime}/root.cpio > options
          grep -Fx 'options zfs zfs_arc_max=67108864' options
          cpio --quiet -i --to-stdout etc/zbm-rs/setup.sh < ${appliance.runtime}/root.cpio > setup
          grep -Fx '/usr/bin/modprobe nvme || exit 1' setup
          cpio --quiet -i --to-stdout etc/hostid < ${appliance.runtime}/root.cpio > hostid
          test "$(od -An -tx1 hostid | tr -d ' \n')" = cdab3412
          grep -F 'spl.spl_hostid=0x1234abcd' ${appliance}/cmdline
          touch $out
        '';
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
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [ cargo rustc rustfmt clippy qemu cpio uv python3 gzip xz zstd patchelf util-linux dosfstools mtools ];
        nativeBuildInputs = [ pkgs.pkg-config ];
        buildInputs = [ pkgs.openssl ];
      };
    };
}
