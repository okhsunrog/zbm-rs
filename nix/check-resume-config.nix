{ self, nixpkgs, pkgs, system }:
let
  evaluate = enabled: overrides: (nixpkgs.lib.nixosSystem {
    inherit system pkgs;
    modules = [ self.nixosModules.default {
      networking.hostId = "1234abcd";
      boot.loader.zbm-rs = {
        enable = true;
        hibernation = {
          enable = enabled;
          swapPartuuid = "12345678-1234-1234-1234-123456789abc";
        };
      };
      boot.zfs.unsafeAllowHibernation = true;
      fileSystems."/" = { device = "tank/root"; fsType = "zfs"; };
      system.stateVersion = "26.05";
    } overrides ];
  }).config;
  off = evaluate false {};
  on = evaluate true {};
  encryptedSettings = {
    boot.loader.zbm-rs.hibernation = {
      swapPartuuid = nixpkgs.lib.mkForce "";
      swap = { type = "luks2"; partuuid = "12345678-1234-1234-1234-123456789abc"; luksDevice = "resume-swap"; };
      credential = { type = "zfs-unlock"; poolGuid = "18446744073709551615"; encryptionRoot = "tank/root"; };
    };
    boot.initrd.luks.devices.resume-swap = {
      device = "/dev/disk/by-uuid/ca55a30a-636e-4e69-9e68-bb36bf380408";
      keyFile = "/etc/zfs/test.key";
    };
    boot.initrd.secrets."/etc/zfs/test.key" = "/etc/zfs/test.key";
  };
  encrypted = evaluate true encryptedSettings;
  encryptedOff = evaluate false encryptedSettings;
  gate = on.boot.initrd.systemd.services.zbm-resume;
  rescue = self.lib.mkImage {
    loaderConfig.resume = encryptedOff.boot.loader.zbm-rs.hibernation.cancellationConfig;
  };
in
assert builtins.elem "nohibernate" (nixpkgs.lib.splitString " " rescue.cmdline);
assert !(builtins.elem "sysctl.kernel.kexec_load_disabled=1" (nixpkgs.lib.splitString " " rescue.cmdline));
assert !(off.boot.initrd.systemd.services ? zbm-resume);
assert off.boot.loader.zbm-rs.hibernation.resumeConfig == {};
assert encryptedOff.boot.loader.zbm-rs.hibernation.resumeConfig == {};
assert encryptedOff.boot.loader.zbm-rs.hibernation.cancellationConfig.role == "cancel-only";
assert encryptedOff.boot.loader.zbm-rs.hibernation.cancellationConfig.device.luks_uuid == "ca55a30a-636e-4e69-9e68-bb36bf380408";
assert encryptedOff.boot.loader.zbm-rs.hibernation.cancellationConfig.credential.pool_guid == "18446744073709551615";
assert !(builtins.elem "hibernate=noresume" off.boot.kernelParams);
assert builtins.elem "hibernate=noresume" on.boot.kernelParams;
assert !(builtins.elem "noresume" on.boot.kernelParams);
assert builtins.elem "zbm-resume.service" on.boot.initrd.systemd.services.zfs-import-tank.requires;
assert builtins.elem "zbm-resume.service" on.boot.initrd.systemd.services.zfs-import-tank.after;
assert builtins.elem "systemd-hibernate-resume.service" on.boot.initrd.systemd.suppressedUnits;
assert builtins.elem "${on.boot.initrd.systemd.package}/lib/systemd/system-generators/systemd-hibernate-resume-generator" on.boot.initrd.systemd.suppressedStorePaths;
assert builtins.elem "initrd-root-fs.target" gate.before;
assert gate.serviceConfig.RemainAfterExit;
assert builtins.all (a: a.assertion) on.assertions;
assert builtins.all (a: a.assertion) encrypted.assertions;
assert encrypted.boot.loader.zbm-rs.hibernation.resumeConfig.device.luks_uuid == "ca55a30a-636e-4e69-9e68-bb36bf380408";
assert encrypted.boot.loader.zbm-rs.hibernation.resumeConfig.device.mapping == "resume-swap";
assert builtins.elem "systemd-cryptsetup@resume\\x2dswap.service" encrypted.boot.initrd.systemd.services.zbm-resume.requires;
assert builtins.elem "initrd-nixos-copy-secrets.service" encrypted.boot.initrd.systemd.services."systemd-cryptsetup@resume\\x2dswap".requires;
pkgs.runCommand "zbm-resume-config-check" {} ''touch $out''
