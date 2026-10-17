{
  self,
  pkgs,
  nixpkgs,
  system,
}:
let
  targetConfig =
    (import "${nixpkgs}/nixos/lib/eval-config.nix" {
      inherit system;
      modules = [
        (import ./nixos-boot.nix { inherit self; })
        "${nixpkgs}/nixos/modules/profiles/qemu-guest.nix"
        "${nixpkgs}/nixos/modules/testing/test-instrumentation.nix"
        ({ lib, ... }: {
          networking.hostName = "native-nixos";
          networking.hostId = "00bab10c";
          boot.loader.zbm-rs.enable = true;
          boot.initrd.secrets."/etc/zfs/test.key" = "/etc/zfs/test.key";
          boot.initrd.systemd.services.zfs-import-testpool = {
            requires = [ "initrd-nixos-copy-secrets.service" ];
            after = [ "initrd-nixos-copy-secrets.service" ];
          };
          boot.zfs.requestEncryptionCredentials = [ "testpool" ];
          boot.zfs.devNodes = "/dev/disk/by-path";
          boot.kernelParams = [
            "console=ttyS0"
            "acpi_osi=\"!Windows 2020\""
          ];
          fileSystems."/" = {
            device = "testpool/nixos/root";
            fsType = "zfs";
            options = [ "zfsutil" ];
          };
          fileSystems."/home" = {
            device = "testpool/nixos/home";
            fsType = "zfs";
            options = [ "zfsutil" ];
            neededForBoot = true;
          };
          users.users.probe = {
            isNormalUser = true;
            uid = 1000;
          };
          environment.systemPackages = [
            pkgs.kexec-tools
            self.packages.${system}.zbm-rs
          ];
          system.extraDependencies = [ imageConfig ];
          environment.etc.generation-marker.text = "original";
          specialisation.updated.configuration.environment.etc.generation-marker.text = lib.mkForce "updated";
          system.stateVersion = "26.05";
        })
      ];
    }).config;
  target = targetConfig.system.build.toplevel;
  loader = self.packages.${system}.zbm-rs;
  imageConfig = pkgs.writeText "native-nixos-plan-config" "{}";
in
pkgs.testers.runNixOSTest {
  name = "zbm-native-nixos-generations";
  globalTimeout = 600;
  nodes.machine = { ... }: {
    networking.hostId = "00bab10c";
    boot.supportedFilesystems = [ "zfs" ];
    virtualisation.memorySize = 3072;
    virtualisation.emptyDiskImages = [ 16000 ];
    virtualisation.additionalPaths = [ target ];
    environment.systemPackages = [
      pkgs.zfs
      pkgs.kexec-tools
      pkgs.nixos-install-tools
      pkgs.jq
      loader
    ];
  };
  testScript = ''
    import json, shlex
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("mkdir -p /etc/zfs; printf disposable-vm-passphrase > /etc/zfs/test.key; chmod 600 /etc/zfs/test.key")
    machine.succeed("zpool create -O mountpoint=none -O overlay=off -O acltype=posixacl -O xattr=sa -O encryption=aes-256-gcm -O keyformat=passphrase -O keylocation=file:///etc/zfs/test.key testpool /dev/vdb")
    machine.succeed("zfs create -u -o mountpoint=none testpool/nixos; zfs create -u -o mountpoint=/ -o canmount=noauto testpool/nixos/root; zfs create -u -o mountpoint=/home -o canmount=noauto testpool/nixos/home")
    machine.succeed("mkdir /mnt; mount -t zfs -o zfsutil testpool/nixos/root /mnt; mkdir /mnt/home; mount -t zfs -o zfsutil testpool/nixos/home /mnt/home; mkdir -p /mnt/etc/zfs; cp /etc/zfs/test.key /mnt/etc/zfs/test.key")
    machine.succeed("nixos-install --root /mnt --system ${target} --no-root-passwd --no-channel-copy", timeout=300)
    machine.succeed("test ! -e /mnt/boot/vmlinuz-nixos; test ! -e /mnt/boot/nixos-current; zfs snapshot testpool/nixos/root@original")
    def plan(root, dataset, system):
        output = machine.succeed(f"${loader}/bin/zbm-rs --nixos-plans {root} {dataset} ${imageConfig} {system}")
        return json.loads(output)[0]
    def handoff(p):
        line = " ".join('"' + arg + '"' if any(c.isspace() for c in arg) else arg for arg in p["cmdline"])
        machine.succeed("kexec -l " + shlex.quote(p["kernel"]) + " --initrd=" + shlex.quote(p["initrd"]) + " --command-line=" + shlex.quote(line))
    p = plan("/mnt", "testpool/nixos/root", "${target}")
    assert "acpi_osi=!Windows 2020" in p["cmdline"]
    assert "rd.fstab=no" not in p["cmdline"]
    assert p["kernel"].startswith("/mnt/boot/zbm-rs/")
    handoff(p)
    machine.succeed("umount /mnt/home /mnt; zpool export testpool")
    machine.execute("systemctl --no-block kexec", check_return=False)
    machine.connected = False
    machine.connect()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("test $(cat /etc/generation-marker) = original; test $(findmnt -n -o SOURCE /home) = testpool/nixos/home; test $(stat -c %u /home/probe) = 1000")
    machine.fail("journalctl -b --no-pager | grep 'Duplicate entry'")
    machine.succeed("readlink /boot/zbm-rs/nixos/${builtins.baseNameOf target} > /tmp/before; mv /etc/zfs/test.key /etc/zfs/test.key.saved")
    missing = machine.fail("${target}/bin/switch-to-configuration boot 2>&1", timeout=120)
    assert "/etc/zfs/test.key" in missing
    machine.succeed("readlink /boot/zbm-rs/nixos/${builtins.baseNameOf target} > /tmp/after; cmp /tmp/before /tmp/after; mv /etc/zfs/test.key.saved /etc/zfs/test.key")
    updated = "${target}/specialisation/updated"
    machine.succeed(updated + "/bin/switch-to-configuration boot", timeout=120)
    handoff(plan("/", "testpool/nixos/root", updated))
    machine.execute("systemctl --no-block kexec", check_return=False)
    machine.connected = False
    machine.connect()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("test $(cat /etc/generation-marker) = updated; test $(findmnt -n -o SOURCE /home) = testpool/nixos/home")
    # The root's snapshot retains both the store closure and prepared artifacts.
    machine.succeed("zfs clone -o mountpoint=/ -o canmount=noauto testpool/nixos/root@original testpool/nixos/recovery; mkdir -p /mnt/recovery; mount -t zfs -o zfsutil testpool/nixos/recovery /mnt/recovery")
    print(machine.succeed("findmnt -t zfs; ls -ld /mnt/recovery/nix/store/${builtins.baseNameOf target}; ls -l /mnt/recovery/boot/zbm-rs/nixos; ls -l /mnt/recovery/boot/zbm-rs/bundles"))
    p = plan("/mnt/recovery", "testpool/nixos/recovery", "${target}")
    handoff(p)
    machine.succeed("umount /mnt/recovery")
    machine.execute("systemctl --no-block kexec", check_return=False)
    machine.connected = False
    machine.connect()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("test $(findmnt -n -o SOURCE /) = testpool/nixos/recovery; test $(cat /etc/generation-marker) = original; test $(findmnt -n -o SOURCE /home) = testpool/nixos/home")
    machine.shutdown()
  '';
}
