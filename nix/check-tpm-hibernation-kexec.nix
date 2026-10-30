# EFI/PCR5 inheritance and NV protection with forced integrity Lockdown.
# Inputs are public, signed artifacts; keys never enter the derivation or VM.
# This probe is not full RAM restore or firmware Secure Boot acceptance.
{ pkgs, kernelPackages, signedKernel, signedModules, tpmDeviceModel ? "tpm-tis",
  tpmBanks ? [ "sha1" "sha256" ], memoryMapProbe ? false }:
let
  manifest = builtins.fromJSON (builtins.readFile "${signedModules}/zbm-module-manifest.json");
  signedPackages = kernelPackages.extend (_: _: {
    kernel = import ./signed-vm-kernel.nix {
      inherit pkgs signedKernel;
      kernel = kernelPackages.kernel;
    };
  });
in
assert manifest.kernel_store_path == toString kernelPackages.kernel;
assert manifest.module_version == kernelPackages.kernel.modDirVersion;
assert builtins.elem tpmDeviceModel [ "tpm-tis" "tpm-crb" ];
assert tpmBanks != [] && pkgs.lib.unique tpmBanks == tpmBanks
  && builtins.all (bank: builtins.elem bank [ "sha1" "sha256" ]) tpmBanks;
pkgs.testers.runNixOSTest {
  name = "tpm-hibernation-efi-kexec-${tpmDeviceModel}-${pkgs.lib.concatStringsSep "-" tpmBanks}${pkgs.lib.optionalString memoryMapProbe "-firmware-map"}";
  globalTimeout = 600;
  nodes.machine = { lib, ... }: {
    boot.kernelPackages = signedPackages;
    system.modulesTree = lib.mkForce [ signedModules ];
    boot.loader.grub.enable = lib.mkForce false;
    boot.loader.systemd-boot.enable = true;
    boot.loader.systemd-boot.consoleMode = "keep";
    boot.zfs.unsafeAllowHibernation = true;
    boot.kernelParams = lib.optional memoryMapProbe "efi=debug";
    # This is the kernel mechanics probe. Full target IMA/BootAuthorization and
    # the enforced zbm-rs handoff have their own integration gate.
    security.lsm = lib.mkForce [ "landlock" "lockdown" "yama" "bpf" ];
    virtualisation.useBootLoader = true;
    virtualisation.useEFIBoot = true;
    virtualisation.tpm.enable = true;
    virtualisation.tpm.deviceModel = tpmDeviceModel;
    virtualisation.tpm.provisioning = import ./tpm-pcr-provision.nix { inherit pkgs; banks = tpmBanks; };
    virtualisation.memorySize = 1536;
    virtualisation.useNixStoreImage = true;
    virtualisation.writableStore = true;
    virtualisation.mountHostNixStore = false;
    virtualisation.sharedDirectories = lib.mkForce {};
    virtualisation.qemu.enableSharedMemory = false;
    environment.systemPackages = [ pkgs.tpm2-tools pkgs.kexec-tools ];
  };
  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")
    def evidence():
        banks = machine.succeed("printf '%s\\n' /sys/class/tpm/tpm0/pcr-*").splitlines()
        assert sorted(path.rsplit("pcr-", 1)[-1] for path in banks) == sorted(${builtins.toJSON tpmBanks}), banks
        machine.succeed("set -eu; test -d /sys/firmware/efi; test -c /dev/tpm0; grep -q '\\[integrity\\]' /sys/kernel/security/lockdown; grep -qw disk /sys/power/state; ! grep -qw nohibernate /proc/cmdline")
        machine.succeed("journalctl -k -b --no-pager | grep 'PCR5='")
        # Prove that a provisioned kernel index cannot be overwritten by
        # userspace, before and after handoff. This touches only the guest TPM.
        machine.succeed("tpm2_nvreadpublic 0x014c4853")
        status, output = machine.execute("printf rejected > /run/nv-test; tpm2_nvwrite -C o -i /run/nv-test 0x014c4853 2>&1")
        assert status != 0 and "Operation not permitted" in output, (status, output)
    evidence()
    ${pkgs.lib.optionalString memoryMapProbe ''
    firmware_maps = []
    for attempt in range(2):
        print("FIRMWARE_BOOT", attempt)
        print(machine.succeed("od -An -tx4 -j 448 -N 32 /sys/kernel/boot_params/data"))
        print(machine.succeed("dmesg | grep -E 'BIOS-e820:|efi:.*(MEMMAP|memmap|PCR5=)'"))
        firmware_maps.append(machine.succeed("dmesg | grep BIOS-e820:"))
        if attempt == 0:
            machine.shutdown()
            machine.start()
            machine.wait_for_unit("multi-user.target")
            evidence()
    assert firmware_maps[0] == firmware_maps[1], (firmware_maps, "Firmware E820 changed across boots")
    ''}
    machine.succeed("test $(cat /proc/sys/kernel/kexec_load_disabled) = 0")
    # These are the two PCR banks included in the RFC's key provenance.
    before = machine.succeed("tpm2_pcrread ${pkgs.lib.concatMapStringsSep "+" (bank: "${bank}:5") tpmBanks}").strip()
    boot_id = machine.succeed("cat /proc/sys/kernel/random/boot_id").strip()
    # One kernel build, two runtime roles. Only the incoming target disables
    # subsequent loads, through its signed-kernel handoff command line.
    machine.succeed("kexec --kexec-file-syscall -l /run/current-system/kernel --initrd=/run/current-system/initrd --command-line=\"$(cat /proc/cmdline) sysctl.kernel.kexec_load_disabled=1\"")
    machine.execute("systemctl --no-block kexec", check_return=False)
    machine.connected = False
    machine.connect()
    machine.wait_for_unit("multi-user.target")
    assert machine.succeed("cat /proc/sys/kernel/random/boot_id").strip() != boot_id
    evidence()
    after = machine.succeed("tpm2_pcrread ${pkgs.lib.concatMapStringsSep "+" (bank: "${bank}:5") tpmBanks}").strip()
    assert before == after, (before, after)
    machine.succeed("test $(cat /proc/sys/kernel/kexec_load_disabled) = 1")
    machine.fail("sysctl -w kernel.kexec_load_disabled=0")
    status, output = machine.execute("kexec --kexec-file-syscall -l /run/current-system/kernel --initrd=/run/current-system/initrd --reuse-cmdline 2>&1")
    assert status != 0 and "Operation not permitted" in output, (status, output)
  '';
}
