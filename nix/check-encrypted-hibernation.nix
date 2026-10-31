{ self, pkgs, nixpkgs, system,
  kernelPackages ? null, signedKernel ? null, signedModules ? null,
  tpmDeviceModel ? "tpm-crb", negativeCase ? "signature",
  tpmBanks ? [ "sha1" "sha256" ], credentialPoolGuid ? "42",
  targetSigning ? null, enforcedTarget ? false, secretSource ? null,
  restoreBeforeNegative ? true, compressedImage ? false,
  sleepOperation ? "hibernate", extraTargetModules ? [] }:
let
  authenticated = kernelPackages != null;
  composite = sleepOperation == "suspend-then-hibernate";
  coldOnly = builtins.elem negativeCase [ "cold-unavailable" "cold-disabled" ];
  manifest = if authenticated then builtins.fromJSON
    (builtins.readFile "${signedModules}/zbm-module-manifest.json") else null;
  signedPackages = if authenticated then kernelPackages.extend (_: _: {
    kernel = import ./signed-vm-kernel.nix {
      inherit pkgs signedKernel;
      kernel = kernelPackages.kernel;
    };
  }) else null;
  protectedProfile = { lib, ... }: lib.mkIf authenticated {
    boot.kernelPackages = signedPackages;
    system.modulesTree = lib.mkForce [ signedModules ];
    # Kernel/TPM mechanics only; enforced BootAuthorization/IMA and firmware
    # Secure Boot are separate acceptance gates.
    security.lsm = lib.mkForce ([ "landlock" "lockdown" "yama" "bpf" ]
      ++ lib.optional enforcedTarget "ima");
  };
  loader = self.packages.${system}.zbm-rs;
  partuuid = "12345678-1234-1234-1234-123456789abc";
  luksUuid = "ca55a30a-636e-4e69-9e68-bb36bf380408";
  targetConfig = (nixpkgs.lib.nixosSystem {
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
          signing = pkgs.lib.optionalAttrs (targetSigning != null)
            (targetSigning // { enable = true; });
          hibernation = {
            enable = true;
            swap = { type = "luks2"; inherit partuuid; luksDevice = "resume-swap"; };
            # Loader credential resolution is tested separately through the UI.
            credential = { type = "zfs-unlock"; poolGuid = credentialPoolGuid; encryptionRoot = "testpool"; };
          };
        };
        boot.zfs.unsafeAllowHibernation = true;
        systemd.sleep.settings.Sleep = pkgs.lib.mkIf composite {
          AllowSuspendThenHibernate = true;
          HibernateDelaySec = "5s";
        };
        services.logind.settings.Login.HandlePowerKey = pkgs.lib.mkIf composite "ignore";
        # Fault injection only: remove swap after systemd selected the resume
        # location, so its real hibernate attempt fails and falls back to RAM.
        environment.etc."systemd/system-sleep/zbm-test-fail-hibernate" = pkgs.lib.mkIf composite {
          mode = "0755";
          text = ''
            #!${pkgs.runtimeShell}
            if test -e /run/zbm-test-fail-hibernate && test "$SYSTEMD_SLEEP_ACTION" = hibernate; then
              case "$1" in
                pre) ${pkgs.util-linux}/bin/swapoff /dev/mapper/resume-swap ;;
                post) ${pkgs.util-linux}/bin/swapon /dev/mapper/resume-swap ;;
              esac
            fi
          '';
        };
        boot.zfs.devNodes = "/dev/disk/by-path";
        boot.zfs.requestEncryptionCredentials = [ "testpool" ];
        boot.initrd.secrets."/etc/zfs/test.key" = if secretSource == null then "/etc/zfs/test.key" else secretSource;
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
        ] ++ pkgs.lib.optionals composite [ "mem_sleep_default=s2idle" "no_console_suspend" ]
          ++ pkgs.lib.optional (authenticated && !compressedImage) "hibernate=nocompress"
          # Explicit repair boot: firmware still advertises the TPM that this
          # fixture deliberately unbinds. Keep native TPM units, but do not let
          # their generator wait forever for that intentionally absent device.
          ++ pkgs.lib.optional (negativeCase == "cold-unavailable") "systemd.tpm2_wait=0";
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
          requires = [ "test-kexec-disabled-early.service" ]
            ++ pkgs.lib.optional authenticated "test-remove-tpm.service";
          after = [ "test-kexec-disabled-early.service" ]
            ++ pkgs.lib.optional authenticated "test-remove-tpm.service";
          serviceConfig.StandardError = "journal+console";
        };
        boot.initrd.systemd.services.test-remove-tpm = pkgs.lib.mkIf authenticated {
          unitConfig = {
            DefaultDependencies = false;
            ConditionPathExists = "/etc/zbm-rs/test-without-tpm";
          };
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
            StandardOutput = "journal+console";
          };
          script = ''
            set -eu
            device=$(basename "$(readlink -f /sys/class/tpm/tpm0/device)")
            echo "$device" > /sys/class/tpm/tpm0/device/driver/unbind
            test ! -e /sys/class/tpm/tpm0
            echo ZBM_TEST_TPM_REMOVED
          '';
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
        environment.etc."zbm-rs/test-resume-policy.json".source = resumeConfig;
        system.stateVersion = "26.05";
      }
    ] ++ extraTargetModules;
  }).config;
  target = targetConfig.system.build.toplevel;
  resumeConfig = pkgs.writeText "test-resume-policy.json" (builtins.toJSON {
    resume = targetConfig.boot.loader.zbm-rs.hibernation.resumeConfig;
  });
  planConfig = pkgs.writeText "resume-plan-config.json" "{}";
in
assert (signedKernel != null) == authenticated && (signedModules != null) == authenticated;
assert !enforcedTarget || (authenticated && targetSigning != null);
assert !authenticated || manifest.kernel_store_path == toString kernelPackages.kernel;
assert !authenticated || manifest.module_version == kernelPackages.kernel.modDirVersion;
assert builtins.elem tpmDeviceModel [ "tpm-tis" "tpm-crb" ];
assert builtins.elem negativeCase [ "signature" "payload" "tpm-reset" "tpm-unavailable" "cold-unavailable" "cold-disabled" ];
assert negativeCase != "cold-unavailable" || authenticated;
assert !compressedImage || (authenticated && negativeCase == "tpm-unavailable");
assert restoreBeforeNegative || authenticated;
assert builtins.elem sleepOperation [ "hibernate" "suspend-then-hibernate" ];
assert tpmBanks != [] && pkgs.lib.unique tpmBanks == tpmBanks
  && builtins.all (bank: builtins.elem bank [ "sha1" "sha256" ]) tpmBanks;
(pkgs.testers.runNixOSTest {
  name = (if authenticated then "zbm-authenticated-hibernation-${tpmDeviceModel}-${pkgs.lib.concatStringsSep "-" tpmBanks}-${negativeCase}${pkgs.lib.optionalString (!restoreBeforeNegative) "-negative-only"}${pkgs.lib.optionalString compressedImage "-compressed"}"
    else if coldOnly then "zbm-encrypted-cold-disabled" else "zbm-encrypted-hibernation")
    + pkgs.lib.optionalString composite "-suspend-then-hibernate";
  globalTimeout = 1200;
  nodes.machine = { lib, config, ... }: {
    imports = [ protectedProfile ];
    boot.loader.grub.enable = lib.mkIf authenticated (lib.mkForce false);
    boot.loader.systemd-boot.enable = authenticated;
    virtualisation.useBootLoader = authenticated;
    virtualisation.useEFIBoot = authenticated;
    virtualisation.tpm.enable = authenticated;
    virtualisation.tpm.deviceModel = lib.mkIf authenticated tpmDeviceModel;
    virtualisation.tpm.provisioning = lib.mkIf authenticated
      (import ./tpm-pcr-provision.nix { inherit pkgs; banks = tpmBanks; });
    # virtio-fs cannot freeze for hibernation. Use an isolated block store and
    # no shared devices; the installed target never mounts the bootstrap disks.
    virtualisation.useNixStoreImage = true;
    virtualisation.writableStore = true;
    virtualisation.mountHostNixStore = false;
    virtualisation.sharedDirectories = lib.mkForce {};
    virtualisation.qemu.enableSharedMemory = false;
    networking.hostId = "00bab10c";
    boot.supportedFilesystems = [ "zfs" ];
    # Permit exercising the raw snapshot signature parser in this disposable
    # bootstrap. Otherwise ZFS adds nohibernate and open() returns EPERM before
    # the parser is reached. The sleeping target pool stays unimported here.
    boot.zfs.unsafeAllowHibernation = authenticated;
    # Bootstrap owns separate disks and must never act on the target's
    # HibernateLocation EFI variable. Only the selected native gate may resume.
    boot.kernelParams = [ "hibernate=noresume" ];
    boot.initrd.systemd.suppressedUnits = [ "systemd-hibernate-resume.service" "systemd-hibernate-resume@.service" ];
    boot.initrd.systemd.suppressedStorePaths = [
      "${config.boot.initrd.systemd.package}/lib/systemd/system-generators/systemd-hibernate-resume-generator"
    ];
    virtualisation.memorySize = 2048;
    virtualisation.emptyDiskImages = [ 12000 2048 ];
    virtualisation.additionalPaths = [ target resumeConfig ] ++ lib.optionals authenticated [
      ./check-snapshot-signature.py ./tamper-hibernation-signature.py
    ];
    environment.systemPackages = [ pkgs.zfs pkgs.kexec-tools pkgs.nixos-install-tools pkgs.cryptsetup pkgs.lvm2 pkgs.gptfdisk loader ]
      ++ lib.optionals authenticated [ pkgs.tpm2-tools pkgs.uv pkgs.python3 pkgs.cpio pkgs.zstd ];
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
    # Copying the full signed module closure can exceed five minutes under
    # concurrent VM/build I/O. Resume/rejection deadlines below stay separate.
    machine.succeed("nixos-install --root /mnt --system ${target} --no-root-passwd --no-channel-copy", timeout=600)
    plan = json.loads(machine.succeed("${loader}/bin/zbm-rs --nixos-plans /mnt testpool/root ${planConfig} ${target}"))[0]
    # Save exact prepared boot inputs on the bootstrap VM's separate root disk.
    # That filesystem is never mounted by the hibernating target. After sleep,
    # bootstrap must not import testpool or activate its swap before kexec.
    machine.succeed("cp " + shlex.quote(plan["kernel"]) + " /root/resume-kernel; cp " + shlex.quote(plan["initrd"]) + " /root/resume-initrd")
    ${pkgs.lib.optionalString authenticated ''
    machine.succeed("cmp /root/resume-kernel ${signedKernel}")
    ''}
    cmdline = " ".join('"' + a + '"' if any(c.isspace() for c in a) else a for a in plan["cmdline"])
    ${pkgs.lib.optionalString (negativeCase == "cold-disabled") ''
    cmdline += " nohibernate"
    ''}
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
    ${pkgs.lib.optionalString (negativeCase == "cold-unavailable") ''
    machine.succeed("mkdir -p /root/without-tpm/etc/zbm-rs; touch /root/without-tpm/etc/zbm-rs/test-without-tpm; cd /root/without-tpm; find . | cpio -o -H newc | zstd -q >> /root/resume-initrd")
    ''}
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
    ${pkgs.lib.optionalString (authenticated && !coldOnly) ''
    machine.succeed("set -eu; grep -q '\\[integrity\\]' /sys/kernel/security/lockdown; test -c /dev/tpm0; tpm2_nvreadpublic 0x014c4853")
    banks = machine.succeed("printf '%s\\n' /sys/class/tpm/tpm0/pcr-*").splitlines()
    assert sorted(path.rsplit("pcr-", 1)[-1] for path in banks) == sorted(${builtins.toJSON tpmBanks}), banks
    machine.wait_for_console_text("ZBM_TEST_BEFORE_POOL_IMPORT", timeout=10)
    ''}
    machine.succeed("test $(findmnt -n -o SOURCE /) = testpool/root; test -b /dev/mapper/resume-swap; grep -q /dev/dm- /proc/swaps")
    ${pkgs.lib.optionalString coldOnly ''
    machine.succeed("set -eu; ! grep -qw disk /sys/power/state; zpool status -x | grep -q 'all pools are healthy'")
    assert json.loads(machine.succeed("${loader}/bin/zbm-rs --resume-status-config ${resumeConfig}"))["state"] == "inactive"
    machine.wait_for_console_text("Hibernation unavailable; verified empty swap permits cold boot", timeout=10)
    ${pkgs.lib.optionalString authenticated ''
    machine.succeed("test ! -e /sys/class/tpm/tpm0; grep -q '\\[integrity\\]' /sys/kernel/security/lockdown")
    machine.wait_for_console_text("ZBM_TEST_BEFORE_POOL_IMPORT", timeout=10)
    ''}
    ''}
    ${pkgs.lib.optionalString (!coldOnly) ''
    machine.succeed("test $(cat /sys/power/resume) = $(lsblk -dn -o MAJ:MIN /dev/mapper/resume-swap); grep -qw disk /sys/power/state")
    before = machine.succeed("cat /proc/sys/kernel/random/boot_id").strip()
    machine.succeed("printf preserved-in-ram > /run/resume-proof; echo 67108864 > /sys/power/image_size")
    status_command = "${loader}/bin/zbm-rs --resume-status-config ${resumeConfig}"
    record_command = "${targetConfig.systemd.services.systemd-hibernate.preStart}"
    activate_command = "${loader}/bin/zbm-rs --resume-activate-config ${resumeConfig}"
    ${pkgs.lib.optionalString composite (builtins.readFile ./check-suspend-then-hibernate.py)}
    machine.execute("systemctl --no-block ${sleepOperation}", check_return=False)
    ${pkgs.lib.optionalString composite "wake_from_suspend(8)"}
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=180))
    ${pkgs.lib.optionalString compressedImage ''
    assert "Compressing and saving image data" in machine.get_console_log()
    ''}
    ${pkgs.lib.optionalString restoreBeforeNegative ''
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("test $(hostname) != resume-target; test ! -e /dev/mapper/resume-swap; ! zpool list -H -o name | grep -qx testpool")
    machine.succeed("test $(cat /proc/sys/kernel/kexec_load_disabled) = 0")
    machine.succeed(load)
    machine.execute("systemctl --no-block kexec", check_return=False)
    # The target's shell resumes too; it does not emit a new boot handshake.
    machine.wait_for_console_text("PM: hibernation: hibernation exit", timeout=90)
    ${pkgs.lib.optionalString compressedImage ''
    assert "Loading and decompressing image data" in machine.get_console_log()
    ''}
    ${pkgs.lib.optionalString (authenticated && (kernelPackages.kernel.zbmProtectedHibernation.contextHandoffProtocol or null) == 1) ''
    # The message precedes hibernation exit and was already consumed from the
    # driver's wait queue. The complete boot log retains both observations.
    assert "Restored authenticated NULL-primary context" in machine.get_console_log()
    ''}
    machine.wait_for_unit("multi-user.target")
    after = machine.succeed("cat /proc/sys/kernel/random/boot_id").strip()
    assert before == after, (before, after, "cold boot occurred instead of RAM restore")
    machine.wait_until_succeeds("test $(systemctl show -p ActiveState --value systemd-${sleepOperation}.service) = inactive", timeout=30)
    machine.succeed("test $(systemctl show -p Result --value systemd-${sleepOperation}.service) = success")
    assert json.loads(machine.succeed("${loader}/bin/zbm-rs --resume-status-config ${resumeConfig}"))["state"] == "inactive"
    # The restored kernel may have older block-cache pages. Confirm the cleared
    # attempt is actually on disk, rather than accepting a cached header alone.
    machine.succeed("dd if=/dev/mapper/resume-swap of=/run/resume-header-direct bs=4096 count=1 iflag=direct status=none; dd if=/dev/mapper/resume-swap of=/run/resume-header-cached bs=4096 count=1 status=none; cmp /run/resume-header-direct /run/resume-header-cached")
    check_target_kexec_disabled()
    machine.succeed("test $(cat /run/resume-proof) = preserved-in-ram; test $(findmnt -n -o SOURCE /) = testpool/root; zpool status -x | grep -q 'all pools are healthy'")
    machine.succeed("journalctl -b --no-pager | grep -E 'hibernation (entry|exit)' ")
    ${pkgs.lib.optionalString composite ''
    # A second full timeout/restore must work after the manual wake, fallback
    # and first restoration, retaining the same running kernel and TPM context.
    machine.execute("systemctl --no-block ${sleepOperation}", check_return=False)
    wake_from_suspend(8)
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=180))
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("test ! -e /dev/mapper/resume-swap; ! zpool list -H -o name | grep -qx testpool")
    machine.succeed(load)
    restore_start = len(machine.get_console_log())
    machine.execute("systemctl --no-block kexec", check_return=False)
    machine.wait_for_console_text("PM: hibernation: hibernation exit", timeout=90)
    ${pkgs.lib.optionalString authenticated ''
    assert "Restored authenticated NULL-primary context" in machine.get_console_log()[restore_start:]
    ''}
    machine.wait_for_unit("multi-user.target")
    finished()
    ''}
    ''}
    ${pkgs.lib.optionalString authenticated ''
    # Create a second authentic image, then corrupt just its ECDSA signature
    # through LUKS on the isolated bootstrap. Never import the sleeping pool.
    ${pkgs.lib.optionalString restoreBeforeNegative ''
    machine.execute("systemctl --no-block hibernate", check_return=False)
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=180))
    ''}
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("set -eu; test $(hostname) != resume-target; test ! -e /dev/mapper/resume-swap; ! zpool list -H -o name | grep -qx testpool")
    # The raw userspace snapshot interface shares the signature validator; it
    # must not accept an unsigned header. This never requests atomic restore.
    # Its pinned page-layout helper only parses uncompressed swap images.
    ${pkgs.lib.optionalString (!compressedImage) ''
    machine.succeed("cryptsetup open --key-file /etc/zfs/test.key /dev/vdd1 resume-swap")
    machine.succeed("uv run --offline --no-project --python ${pkgs.python3}/bin/python ${./check-snapshot-signature.py} /dev/mapper/resume-swap")
    machine.succeed("cryptsetup close resume-swap")
    ''}
    ${pkgs.lib.optionalString (builtins.elem negativeCase [ "signature" "payload" ]) ''
    machine.succeed("cryptsetup open --key-file /etc/zfs/test.key /dev/vdd1 resume-swap")
    machine.succeed("uv run --offline --no-project --python ${pkgs.python3}/bin/python ${./tamper-hibernation-signature.py} /dev/mapper/resume-swap ${negativeCase}")
    machine.succeed("cryptsetup close resume-swap")
    ''}
    ${pkgs.lib.optionalString (negativeCase == "tpm-reset") ''
    # OVMF randomizes platform auth; this disposable TPM has empty lockout auth.
    machine.succeed("tpm2_clear -T device:/dev/tpmrm0 -c l")
    ''}
    ${pkgs.lib.optionalString (negativeCase == "tpm-unavailable") ''
    # A fixture-only appended cpio enables the pre-resume driver removal. The
    # mechanics fixture deliberately omits IMA; this is not an authorized loader
    # handoff or a production mechanism for changing signed inputs.
    machine.succeed("cp /root/resume-initrd /root/resume-initrd.clean; mkdir -p /root/without-tpm/etc/zbm-rs; touch /root/without-tpm/etc/zbm-rs/test-without-tpm; cd /root/without-tpm; find . | cpio -o -H newc | zstd -q >> /root/resume-initrd")
    ''}
    machine.succeed(load)
    console_start = len(machine.get_console_log())
    machine.execute("systemctl --no-block kexec", check_return=False)
    machine.wait_for_console_text("${if builtins.elem negativeCase [ "signature" "payload" ] then "Image signature verification failed" else if negativeCase == "tpm-reset" then "Image signing key was not created by the kernel" else "ZBM_TEST_TPM_REMOVED"}", timeout=90)
    ${pkgs.lib.optionalString (negativeCase != "tpm-unavailable") ''
    machine.wait_for_console_text("Kernel resume returned without restoring the saved session", timeout=30)
    ''}
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=45))
    # This marker is a required predecessor of the real ZFS import service.
    assert "ZBM_TEST_BEFORE_POOL_IMPORT" not in machine.get_console_log()[console_start:]
    # Linux consumed its image signature before verification failed. The durable
    # attempt must still block a second boot, including after a full power cycle.
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("test ! -e /dev/mapper/resume-swap; ! zpool list -H -o name | grep -qx testpool")
    machine.succeed("cryptsetup open --key-file /etc/zfs/test.key /dev/vdd1 resume-swap")
    failed = json.loads(machine.succeed("${loader}/bin/zbm-rs --resume-status-config ${resumeConfig}"))
    ${pkgs.lib.optionalString (negativeCase != "tpm-unavailable") ''
    assert failed["state"] == "unavailable" and "Previous resume attempt" in failed["reason"], failed
    ''}
    ${pkgs.lib.optionalString (negativeCase == "tpm-unavailable") ''
    assert failed["state"] == "pending", failed
    machine.succeed("cp /root/resume-initrd.clean /root/resume-initrd")
    ''}
    machine.succeed("cryptsetup close resume-swap")
    machine.succeed(load)
    console_start = len(machine.get_console_log())
    machine.execute("systemctl --no-block kexec", check_return=False)
    ${pkgs.lib.optionalString (negativeCase != "tpm-unavailable") ''
    machine.wait_for_console_text("Previous resume did not complete", timeout=90)
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=45))
    assert "ZBM_TEST_BEFORE_POOL_IMPORT" not in machine.get_console_log()[console_start:]
    ''}
    ${pkgs.lib.optionalString (negativeCase == "tpm-unavailable") ''
    machine.wait_for_console_text("PM: hibernation: hibernation exit", timeout=90)
    machine.wait_for_unit("multi-user.target")
    assert machine.succeed("cat /proc/sys/kernel/random/boot_id").strip() == before
    machine.succeed("test $(cat /run/resume-proof) = preserved-in-ram; zpool status -x | grep -q 'all pools are healthy'")
    ${pkgs.lib.optionalString restoreBeforeNegative ''
    # Repeat once more after the TPM-absence recovery. This exercises context
    # inheritance from a kernel that has itself already resumed twice.
    machine.wait_until_succeeds("test $(systemctl show -p ActiveState --value systemd-hibernate.service) = inactive", timeout=30)
    machine.execute("systemctl --no-block hibernate", check_return=False)
    machine.wait_for_shutdown(timeout=datetime.timedelta(seconds=180))
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("test ! -e /dev/mapper/resume-swap; ! zpool list -H -o name | grep -qx testpool")
    machine.succeed(load)
    restore_start = len(machine.get_console_log())
    machine.execute("systemctl --no-block kexec", check_return=False)
    machine.wait_for_console_text("PM: hibernation: hibernation exit", timeout=90)
    ${pkgs.lib.optionalString ((kernelPackages.kernel.zbmProtectedHibernation.contextHandoffProtocol or null) == 1) ''
    assert "Restored authenticated NULL-primary context" in machine.get_console_log()[restore_start:]
    ''}
    machine.wait_for_unit("multi-user.target")
    assert machine.succeed("cat /proc/sys/kernel/random/boot_id").strip() == before
    machine.succeed("test $(cat /run/resume-proof) = preserved-in-ram; zpool status -x | grep -q 'all pools are healthy'")
    check_target_kexec_disabled()
    machine.wait_until_succeeds("test $(systemctl show -p ActiveState --value systemd-hibernate.service) = inactive", timeout=30)
    assert json.loads(machine.succeed("${loader}/bin/zbm-rs --resume-status-config ${resumeConfig}"))["state"] == "inactive"
    ''}
    ''}
    ''}
    ''}
  '';
}) // {
  # The runtime-only protected-chain test prepares the same native target and
  # signs its final inputs outside Nix after learning the disposable pool GUID.
  nativeTarget = target;
  nativeTargetConfig = targetConfig;
  nativeResumeConfig = resumeConfig;
}
