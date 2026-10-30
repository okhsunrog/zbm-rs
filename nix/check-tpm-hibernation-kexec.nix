# EFI/PCR5 inheritance and NV protection with forced integrity Lockdown.
# Inputs are public, signed artifacts; keys never enter the derivation or VM.
# This probe is not full RAM restore or firmware Secure Boot acceptance.
{ pkgs, kernelPackages, signedKernel, signedModules }:
let
  manifest = builtins.fromJSON (builtins.readFile "${signedModules}/zbm-module-manifest.json");
in
assert manifest.kernel_store_path == toString kernelPackages.kernel;
assert manifest.module_version == kernelPackages.kernel.modDirVersion;
pkgs.testers.runNixOSTest {
  name = "tpm-hibernation-efi-kexec-probe";
  globalTimeout = 600;
  nodes.machine = { lib, ... }: {
    boot.kernelPackages = kernelPackages;
    system.modulesTree = lib.mkForce [ signedModules ];
    system.systemBuilderCommands = lib.mkAfter ''
      rm "$out/kernel"
      ln -s ${signedKernel} "$out/kernel"
    '';
    boot.loader.grub.enable = lib.mkForce false;
    boot.loader.systemd-boot.enable = true;
    boot.loader.systemd-boot.consoleMode = "keep";
    boot.zfs.unsafeAllowHibernation = true;
    # This is the kernel mechanics probe. Full target IMA/BootAuthorization and
    # the enforced zbm-rs handoff have their own integration gate.
    security.lsm = lib.mkForce [ "landlock" "lockdown" "yama" "bpf" ];
    virtualisation.useBootLoader = true;
    virtualisation.useEFIBoot = true;
    virtualisation.tpm.enable = true;
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
        machine.succeed("set -eu; test -d /sys/firmware/efi; test -c /dev/tpm0; grep -q '\\[integrity\\]' /sys/kernel/security/lockdown; grep -qw disk /sys/power/state; ! grep -qw nohibernate /proc/cmdline")
        machine.succeed("journalctl -k -b --no-pager | grep 'PCR5='")
        # Prove that a provisioned kernel index cannot be overwritten by
        # userspace, before and after handoff. This touches only the guest TPM.
        machine.succeed("tpm2_nvreadpublic 0x014c4853")
        status, output = machine.execute("printf rejected > /run/nv-test; tpm2_nvwrite -C o -i /run/nv-test 0x014c4853 2>&1")
        assert status != 0 and "Operation not permitted" in output, (status, output)
    evidence()
    machine.succeed("test $(cat /proc/sys/kernel/kexec_load_disabled) = 0")
    # These are the two PCR banks included in the RFC's key provenance.
    before = machine.succeed("tpm2_pcrread sha1:5+sha256:5").strip()
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
    after = machine.succeed("tpm2_pcrread sha1:5+sha256:5").strip()
    assert before == after, (before, after)
    machine.succeed("test $(cat /proc/sys/kernel/kexec_load_disabled) = 1")
    machine.fail("sysctl -w kernel.kexec_load_disabled=0")
    status, output = machine.execute("kexec --kexec-file-syscall -l /run/current-system/kernel --initrd=/run/current-system/initrd --reuse-cmdline 2>&1")
    assert status != 0 and "Operation not permitted" in output, (status, output)
  '';
}
