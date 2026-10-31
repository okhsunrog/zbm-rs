# Build only .driver, then run it outside Nix with disposable runtime key paths.
# No signer or enrollment key is a derivation input or guest file.
{ self, pkgs }:
let
  tools = pkgs.symlinkJoin {
    name = "protected-hibernation-runtime-tools";
    paths = [ pkgs.nix pkgs.openssh pkgs.openssl pkgs.uv pkgs.python3
      pkgs.cpio pkgs.gnutar pkgs.zstd pkgs.sbsigntool
      pkgs.python3Packages.virt-firmware ];
  };
  test = pkgs.testers.runNixOSTest {
    name = "protected-hibernation-runtime";
    globalTimeout = 3600;
    nodes.machine = { lib, config, ... }: {
      networking.hostId = "00bab10c";
      boot.supportedFilesystems = [ "zfs" ];
      boot.loader.grub.enable = lib.mkForce false;
      boot.loader.systemd-boot.enable = true;
      boot.initrd.systemd.suppressedUnits = [ "systemd-hibernate-resume.service" "systemd-hibernate-resume@.service" ];
      boot.initrd.systemd.suppressedStorePaths = [
        "${config.boot.initrd.systemd.package}/lib/systemd/system-generators/systemd-hibernate-resume-generator"
      ];
      boot.kernelParams = [ "hibernate=noresume" ];
      virtualisation = {
        useBootLoader = true;
        useEFIBoot = true;
        useNixStoreImage = true;
        writableStore = true;
        mountHostNixStore = false;
        sharedDirectories = lib.mkForce {};
        qemu.enableSharedMemory = false;
        tpm = {
          enable = true;
          deviceModel = "tpm-crb";
          provisioning = import ./tpm-pcr-provision.nix { inherit pkgs; banks = [ "sha256" ]; };
        };
        memorySize = 2048;
        diskSize = 8192;
        emptyDiskImages = [ 12000 2048 ];
        forwardPorts = [{ from = "host"; host.address = "127.0.0.1"; host.port = 22941; guest.port = 22; }];
      };
      services.openssh = {
        enable = true;
        settings = { PasswordAuthentication = false; KbdInteractiveAuthentication = false; PermitRootLogin = "prohibit-password"; };
      };
      environment.systemPackages = [ pkgs.zfs pkgs.cryptsetup pkgs.gptfdisk pkgs.efibootmgr ];
    };
    testScript = ''
      import importlib.util, sys
      sys.dont_write_bytecode = True
      spec = importlib.util.spec_from_file_location("protected_resume", "${self}/nix/protected-hibernation-runtime.py")
      assert spec is not None and spec.loader is not None
      runtime = importlib.util.module_from_spec(spec)
      spec.loader.exec_module(runtime)
      runtime.run(machine, source="${self}", tools="${tools}")
    '';
  };
in test.driver
