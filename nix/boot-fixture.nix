# Disposable NixOS roots for exact-generation kexec acceptance.
{ pkgs, nixpkgs, system, kernelPackages ? pkgs.linuxPackages
, extraNixosModules ? [] }:
let
  generation = number: nixpkgs.lib.nixosSystem {
    inherit system;
    modules = [ ({ lib, ... }: {
      boot.initrd.systemd.enable = true;
      system.stateVersion = "26.05";
      networking.hostName = "zbm-fixture";
      networking.hostId = "01020304";
      networking.useDHCP = false;
      boot.kernelPackages = kernelPackages;
      boot.loader.grub.enable = false;
      boot.supportedFilesystems = [ "zfs" ];
      boot.zfs.forceImportRoot = false;
      boot.initrd.availableKernelModules = [ "virtio_pci" "virtio_blk" "tpm_crb" ];
      boot.initrd.systemd.services.zbm-failure-diagnostics = {
        wantedBy = [ "emergency.target" ];
        unitConfig.DefaultDependencies = false;
        serviceConfig.Type = "oneshot";
        script = ''
          echo ZBM_INITRD_FAILURE > /dev/ttyS0
          journalctl -b --no-pager -u sysroot.mount > /dev/ttyS0
          cat /run/systemd/generator/sysroot.mount > /dev/ttyS0
          cat /proc/cmdline > /dev/ttyS0
        '';
        path = [ pkgs.systemd pkgs.coreutils ];
      };
      boot.kernelParams = [ "console=tty0" "console=ttyS0,115200" "zbm.fixture=${toString number}"
        "systemd.show_status=yes" "rd.systemd.show_status=yes" "rd.systemd.log_level=info" ];
      fileSystems."/" = { device = "zbm_fixture/nixos"; fsType = "zfs"; };
      services.zfs.autoScrub.enable = false;
      documentation.enable = false;
      documentation.nixos.enable = false;
      services.udisks2.enable = false;
      services.nscd.enable = false;
      system.nssModules = lib.mkForce [];
      systemd.services.systemd-boot-random-seed.enable = false;
      environment.defaultPackages = lib.mkForce [];
      systemd.services.zbm-boot-proof = {
        wantedBy = [ "multi-user.target" ];
        after = [ "multi-user.target" ];
        # Avoid the target's implicit ordering before its wanted service.
        unitConfig.DefaultDependencies = false;
        serviceConfig.Type = "oneshot";
        script = ''
          ${pkgs.systemd}/bin/systemctl is-active --quiet multi-user.target
          test "$(findmnt -n -o FSTYPE /)" = zfs
          actual_root="$(findmnt -n -o SOURCE /)"
          case "$actual_root" in
            zbm_fixture/zbm-rs-*)
              test "$(cat /snapshot-proof)" = snapshot-original
              echo clone-write > /snapshot-proof
              test "$(cat /snapshot-proof)" = clone-write
              mkdir -p /run/original-root /run/source-snapshot
              mount -t zfs -o ro zbm_fixture/nixos /run/original-root
              mount -t zfs -o ro zbm_fixture/nixos@known-good /run/source-snapshot
              test "$(cat /run/original-root/snapshot-proof)" = live-changed
              test "$(cat /run/source-snapshot/snapshot-proof)" = snapshot-original
              umount /run/source-snapshot /run/original-root
              ;;
            zbm_fixture/nixos) ;;
            *) exit 1 ;;
          esac
          case " $(cat /proc/cmdline) " in
            *" zbm.fixture=${toString number} "*) ;;
            *) exit 1 ;;
          esac
          if test -e /dev/tpmrm0; then
            echo ZBM_TARGET_TPM_DIAGNOSTICS > /dev/ttyS0
            ${pkgs.systemd}/bin/journalctl -b --no-pager \
              -u systemd-tpm2-setup-early.service -u systemd-tpm2-setup.service > /dev/ttyS0
            ${pkgs.systemd}/bin/systemctl is-active --quiet systemd-tpm2-setup.service
            test -s /var/lib/systemd/tpm2-srk-public-key.pem
            echo ZBM_TARGET_TPM_SRK_READY > /dev/ttyS0
          fi
          echo "ZBM_BOOT_SUCCESS generation=${toString number} system=$(readlink -f /run/current-system) root=$actual_root boot_id=$(cat /proc/sys/kernel/random/boot_id)" > /dev/ttyS0
          ${pkgs.systemd}/bin/systemctl poweroff
        '';
        path = [ pkgs.util-linux pkgs.coreutils ];
      };
    }) ] ++ extraNixosModules;
  };
  first = (generation 1).config.system.build.toplevel;
  second = (generation 2).config.system.build.toplevel;
  closure = pkgs.closureInfo { rootPaths = [ first second ]; };
in pkgs.runCommand "zbm-nixos-boot-fixture" { nativeBuildInputs = [ pkgs.gnutar ]; } ''
  mkdir -p root/nix/var/nix/profiles $out
  ln -s ${first} root/nix/var/nix/profiles/system-1-link
  ln -s ${second} root/nix/var/nix/profiles/system-2-link
  ln -s system-2-link root/nix/var/nix/profiles/system
  tar -cf $out/root.tar -C root .
  sed 's,^/,,' ${closure}/store-paths > store-paths
  tar -rf $out/root.tar -C / -T store-paths
  printf '%s\n' ${first} > $out/generation-1
  printf '%s\n' ${second} > $out/generation-2
''
