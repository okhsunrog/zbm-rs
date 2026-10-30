{ self, pkgs, nixpkgs, system,
  kernelPackages ? null, signedKernel ? null, signedModules ? null }:
let
  authenticated = kernelPackages != null;
  manifest = if authenticated then builtins.fromJSON
    (builtins.readFile "${signedModules}/zbm-module-manifest.json") else null;
  protectedProfile = { lib, ... }: lib.mkIf authenticated {
    boot.kernelPackages = kernelPackages;
    system.modulesTree = lib.mkForce [ signedModules ];
    system.systemBuilderCommands = lib.mkAfter ''
      rm "$out/kernel"
      ln -s ${signedKernel} "$out/kernel"
    '';
    # Kernel/TPM mechanics only; enforced BootAuthorization/IMA and firmware
    # Secure Boot are separate acceptance gates.
    security.lsm = lib.mkForce [ "landlock" "lockdown" "yama" "bpf" ];
  };
  loader = self.packages.${system}.zbm-rs;
  partuuid = "12345678-1234-1234-1234-123456789abc";
  luksUuid = "ca55a30a-636e-4e69-9e68-bb36bf380408";
  target = (nixpkgs.lib.nixosSystem {
    inherit system;
    modules = [
      protectedProfile
      (import ./nixos-boot.nix { inherit self; })
      "${nixpkgs}/nixos/modules/profiles/qemu-guest.nix"
      "${nixpkgs}/nixos/modules/testing/test-instrumentation.nix"
      {
        networking.hostName = "resume-target";
        networking.hostId = "00bab10c";
        boot.loader.zbm-rs = {
          enable = true;
          hibernation = {
            enable = true;
            swap = { type = "luks2"; inherit partuuid; luksDevice = "resume-swap"; };
            # Loader credential resolution is tested separately through the UI.
            credential = { type = "zfs-unlock"; poolGuid = "42"; encryptionRoot = "testpool"; };
          };
        };
        boot.zfs.unsafeAllowHibernation = true;
        boot.zfs.devNodes = "/dev/disk/by-path";
        boot.zfs.requestEncryptionCredentials = [ "testpool" ];
        boot.initrd.secrets."/etc/zfs/test.key" = "/etc/zfs/test.key";
        boot.initrd.luks.devices.resume-swap = {
          device = "/dev/disk/by-uuid/${luksUuid}";
          keyFile = "/etc/zfs/test.key";
        };
        boot.initrd.systemd.services.zfs-import-testpool = {
          requires = [ "initrd-nixos-copy-secrets.service" ]
            ++ pkgs.lib.optional authenticated "test-before-pool-import.service";
          after = [ "initrd-nixos-copy-secrets.service" ]
            ++ pkgs.lib.optional authenticated "test-before-pool-import.service";
        };
        # A negative fixture: even an early read-only pool import must trip the
        # target gate. This service is not part of the production module.
        boot.initrd.systemd.services.test-import-before-resume = {
          wantedBy = [ "initrd.target" ];
          before = [ "zbm-resume.service" ];
          requires = [ "dev-vdc.device" ];
          after = [ "dev-vdc.device" "systemd-modules-load.service" ];
          unitConfig.ConditionKernelCommandLine = "zbm.test.import-before-resume";
          serviceConfig = {
            Type = "oneshot";
            ExecStartPre = "${pkgs.kmod}/bin/modprobe zfs";
            ExecStart = "${pkgs.zfs}/bin/zpool import -N -o readonly=on -o cachefile=none testpool";
          };
        };
        # The target has the same kexec-capable kernel as the bootstrap. Only
        # its command line disables further loads, before any userspace runs.
        boot.kernelParams = [
          "console=ttyS0" "zfs.zfs_arc_max=134217728"
          "sysctl.kernel.kexec_load_disabled=1"
        ] ++ pkgs.lib.optional authenticated "hibernate=nocompress";
        boot.initrd.systemd.services.test-kexec-disabled-early = {
          wantedBy = [ "initrd.target" ];
          before = [ "systemd-udevd.service" "zbm-resume.service" ];
          unitConfig.DefaultDependencies = false;
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
            StandardOutput = "journal+console";
          };
          script = ''
            set -eu
            test "$(< /proc/sys/kernel/kexec_load_disabled)" = 1
            test "$(< /sys/kernel/kexec_loaded)" = 0
            test "$(< /sys/kernel/kexec_crash_loaded)" = 0
            printf 'verified-before-udev-and-resume\n' > /run/kexec-disabled-early
            echo ZBM_TARGET_KEXEC_DISABLED_EARLY
          '';
        };
        boot.initrd.systemd.services.zbm-resume = {
          requires = [ "test-kexec-disabled-early.service" ];
          after = [ "test-kexec-disabled-early.service" ];
          serviceConfig.StandardError = "journal+console";
        };
        boot.initrd.systemd.services.test-before-pool-import = pkgs.lib.mkIf authenticated {
          requires = [ "zbm-resume.service" ];
          after = [ "zbm-resume.service" ];
          serviceConfig = {
            Type = "oneshot";
            StandardOutput = "journal+console";
          };
          script = "echo ZBM_TEST_BEFORE_POOL_IMPORT";
        };
        fileSystems."/" = { device = "testpool/root"; fsType = "zfs"; options = [ "zfsutil" ]; };
        swapDevices = [{ device = "/dev/mapper/resume-swap"; }];
        environment.systemPackages = [ pkgs.kexec-tools pkgs.cryptsetup pkgs.lvm2 loader ]
          ++ pkgs.lib.optional authenticated pkgs.tpm2-tools;
        system.stateVersion = "26.05";
      }
    ];
  }).config.system.build.toplevel;
  planConfig = pkgs.writeText "resume-plan-config.json" "{}";
in
assert (signedKernel != null) == authenticated && (signedModules != null) == authenticated;
assert !authenticated || manifest.kernel_store_path == toString kernelPackages.kernel;
assert !authenticated || manifest.module_version == kernelPackages.kernel.modDirVersion;
pkgs.testers.runNixOSTest {
  name = if authenticated then "zbm-tpm-authenticated-hibernation" else "zbm-encrypted-hibernation";
  globalTimeout = 900;
  nodes.machine = { lib, ... }: {
    imports = [ protectedProfile ];
    boot.loader.grub.enable = lib.mkIf authenticated (lib.mkForce false);
    boot.loader.systemd-boot.enable = authenticated;
    virtualisation.useBootLoader = authenticated;
    virtualisation.useEFIBoot = authenticated;
    virtualisation.tpm.enable = authenticated;
    # virtio-fs cannot freeze for hibernation. Use an isolated block store and
    # no shared devices; the installed target never mounts the bootstrap disks.
    virtualisation.useNixStoreImage = true;
    virtualisation.writableStore = true;
    virtualisation.mountHostNixStore = false;
    virtualisation.sharedDirectories = lib.mkForce {};
    virtualisation.qemu.enableSharedMemory = false;
    networking.hostId = "00bab10c";
    boot.supportedFilesystems = [ "zfs" ];
    virtualisation.memorySize = 2048;
    virtualisation.emptyDiskImages = [ 12000 2048 ];
    virtualisation.additionalPaths = [ target ];
    environment.systemPackages = [ pkgs.zfs pkgs.kexec-tools pkgs.nixos-install-tools pkgs.cryptsetup pkgs.lvm2 pkgs.gptfdisk loader ]
      ++ lib.optionals authenticated [ pkgs.tpm2-tools pkgs.uv pkgs.python3 ];
  };
  testScript = ''
    import json, shlex, datetime
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("test $(cat /proc/sys/kernel/kexec_load_disabled) = 0")
    machine.succeed("mkdir -p /etc/zfs; printf disposable-vm-passphrase > /etc/zfs/test.key; chmod 600 /etc/zfs/test.key")
    machine.succeed("sgdisk --zap-all --new=1:2048:0 --partition-guid=1:${partuuid} /dev/vdd; udevadm settle")
    machine.succeed("cryptsetup luksFormat --batch-mode --type luks2 --pbkdf pbkdf2 --pbkdf-force-iterations 1000 --uuid ${luksUuid} --key-file /etc/zfs/test.key /dev/vdd1; cryptsetup open --key-file /etc/zfs/test.key /dev/vdd1 resume-swap; mkswap /dev/mapper/resume-swap; cryptsetup close resume-swap")
    machine.succeed("zpool create -o cachefile=none -O mountpoint=none -O encryption=aes-256-gcm -O keyformat=passphrase -O keylocation=file:///etc/zfs/test.key testpool /dev/vdc; zfs create -u -o mountpoint=/ -o canmount=noauto testpool/root; mkdir /mnt; mount -t zfs -o zfsutil testpool/root /mnt; mkdir -p /mnt/etc/zfs; cp /etc/zfs/test.key /mnt/etc/zfs/test.key")
    machine.succeed("nixos-install --root /mnt --system ${target} --no-root-passwd --no-channel-copy", timeout=300)
    plan = json.loads(machine.succeed("${loader}/bin/zbm-rs --nixos-plans /mnt testpool/root ${planConfig} ${target}"))[0]
    # Save exact prepared boot inputs on the bootstrap VM's separate root disk.
    # That filesystem is never mounted by the hibernating target. After sleep,
    # bootstrap must not import testpool or activate its swap before kexec.
    machine.succeed("cp " + shlex.quote(plan["kernel"]) + " /root/resume-kernel; cp " + shlex.quote(plan["initrd"]) + " /root/resume-initrd")
    cmdline = " ".join('"' + a + '"' if any(c.isspace() for c in a) else a for a in plan["cmdline"])
    load_prefix = "kexec ${pkgs.lib.optionalString authenticated "--kexec-file-syscall "}-l /root/resume-kernel --initrd=/root/resume-initrd --command-line="
    load = load_prefix + shlex.quote(cmdline)
    machine.succeed("umount /mnt; zpool export testpool")
    machine.succeed(load_prefix + shlex.quote(cmdline + " zbm.test.import-before-resume"))
    machine.execute("systemctl --no-block kexec", check_return=False)
    machine.wait_for_console_text("A ZFS pool was imported before resume", timeout=90)
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=45))
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("test ! -e /dev/mapper/resume-swap; ! zpool list -H -o name | grep -qx testpool")
    machine.succeed("test $(cat /proc/sys/kernel/kexec_load_disabled) = 0")
    machine.succeed(load)
    machine.execute("systemctl --no-block kexec", check_return=False)
    machine.connected = False
    machine.connect()
    machine.wait_for_unit("multi-user.target")
    def check_target_kexec_disabled():
        machine.succeed("set -eu; test $(cat /proc/sys/kernel/kexec_load_disabled) = 1; test $(cat /sys/kernel/kexec_loaded) = 0; test $(cat /run/kexec-disabled-early) = verified-before-udev-and-resume")
        machine.fail("sysctl -w kernel.kexec_load_disabled=0")
        for mode in ["-c", "-s"]:
            status, output = machine.execute("kexec " + mode + " -l /run/current-system/kernel --initrd=/run/current-system/initrd --reuse-cmdline 2>&1")
            assert status != 0 and "Operation not permitted" in output, (mode, status, output)
        machine.succeed("set -eu; test $(cat /proc/sys/kernel/kexec_load_disabled) = 1; test $(cat /sys/kernel/kexec_loaded) = 0")
    check_target_kexec_disabled()
    ${pkgs.lib.optionalString authenticated ''
    machine.succeed("set -eu; grep -q '\\[integrity\\]' /sys/kernel/security/lockdown; test -c /dev/tpm0; tpm2_nvreadpublic 0x014c4853")
    machine.wait_for_console_text("ZBM_TEST_BEFORE_POOL_IMPORT", timeout=10)
    ''}
    machine.succeed("test $(findmnt -n -o SOURCE /) = testpool/root; test -b /dev/mapper/resume-swap; grep -q /dev/dm- /proc/swaps")
    machine.succeed("test $(cat /sys/power/resume) = $(lsblk -dn -o MAJ:MIN /dev/mapper/resume-swap); grep -qw disk /sys/power/state")
    before = machine.succeed("cat /proc/sys/kernel/random/boot_id").strip()
    machine.succeed("printf preserved-in-ram > /run/resume-proof; echo 67108864 > /sys/power/image_size")
    machine.execute("systemctl --no-block hibernate", check_return=False)
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=180))
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("test $(hostname) != resume-target; test ! -e /dev/mapper/resume-swap; ! zpool list -H -o name | grep -qx testpool")
    machine.succeed("test $(cat /proc/sys/kernel/kexec_load_disabled) = 0")
    machine.succeed(load)
    machine.execute("systemctl --no-block kexec", check_return=False)
    # The target's shell resumes too; it does not emit a new boot handshake.
    machine.wait_for_console_text("PM: hibernation: hibernation exit", timeout=90)
    machine.wait_for_unit("multi-user.target")
    after = machine.succeed("cat /proc/sys/kernel/random/boot_id").strip()
    assert before == after, (before, after, "cold boot occurred instead of RAM restore")
    check_target_kexec_disabled()
    machine.succeed("test $(cat /run/resume-proof) = preserved-in-ram; test $(findmnt -n -o SOURCE /) = testpool/root; zpool status -x | grep -q 'all pools are healthy'")
    machine.succeed("journalctl -b --no-pager | grep -E 'hibernation (entry|exit)' ")
    ${pkgs.lib.optionalString authenticated ''
    # Create a second authentic image, then corrupt just its ECDSA signature
    # through LUKS on the isolated bootstrap. Never import the sleeping pool.
    machine.execute("systemctl --no-block hibernate", check_return=False)
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=180))
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("set -eu; test $(hostname) != resume-target; test ! -e /dev/mapper/resume-swap; ! zpool list -H -o name | grep -qx testpool")
    machine.succeed("cryptsetup open --key-file /etc/zfs/test.key /dev/vdd1 resume-swap")
    machine.succeed("uv run --offline --no-project --python ${pkgs.python3}/bin/python ${./tamper-hibernation-signature.py} /dev/mapper/resume-swap")
    machine.succeed("cryptsetup close resume-swap")
    machine.succeed(load)
    console_start = len(machine.get_console_log())
    machine.execute("systemctl --no-block kexec", check_return=False)
    machine.wait_for_console_text("Image signature verification failed", timeout=90)
    machine.wait_for_console_text("Kernel resume returned without restoring the saved session", timeout=30)
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=45))
    # This marker is a required predecessor of the real ZFS import service.
    assert "ZBM_TEST_BEFORE_POOL_IMPORT" not in machine.get_console_log()[console_start:]
    ''}
  '';
}
