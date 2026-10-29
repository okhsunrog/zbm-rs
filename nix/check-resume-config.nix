{ self, nixpkgs, pkgs, system }:
let
  evaluate = enabled: (nixpkgs.lib.nixosSystem {
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
    } ];
  }).config;
  off = evaluate false;
  on = evaluate true;
  gate = on.boot.initrd.systemd.services.zbm-resume;
in
assert !(off.boot.initrd.systemd.services ? zbm-resume);
assert !(builtins.elem "hibernate=noresume" off.boot.kernelParams);
assert builtins.elem "hibernate=noresume" on.boot.kernelParams;
assert !(builtins.elem "noresume" on.boot.kernelParams);
assert builtins.elem "zbm-resume.service" on.boot.initrd.systemd.services.zfs-import-tank.requires;
assert builtins.elem "zbm-resume.service" on.boot.initrd.systemd.services.zfs-import-tank.after;
assert builtins.elem "systemd-hibernate-resume.service" on.boot.initrd.systemd.suppressedUnits;
assert builtins.elem "initrd-root-fs.target" gate.before;
assert gate.serviceConfig.RemainAfterExit;
assert builtins.all (a: a.assertion) on.assertions;
pkgs.runCommand "zbm-resume-config-check" {} ''touch $out''
