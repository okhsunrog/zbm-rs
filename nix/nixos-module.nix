{ self }:
{ config, lib, pkgs, ... }:
let
  cfg = config.programs.zbm-rs;
  image = import ./image.nix {
    inherit pkgs;
    loaderConfig = {
      ui = { timeout_secs = cfg.timeout; show_snapshots = cfg.ui.showSnapshots; title = cfg.ui.title; };
      manager.restart_limit = cfg.manager.restartLimit;
      zfs.import_policy = cfg.zfs.importPolicy;
      nixos.generation_limit = cfg.configurationLimit;
      kernel_args = cfg.kernelArgs;
    };
    source = self;
    zfsSource = self.inputs.zfskit;
    kernelPackages = config.boot.kernelPackages;
    zfsUserspace = config.boot.zfs.package;
    profile = cfg.profile;
    hardwareManifest = cfg.hardwareManifest;
    extraModules = config.boot.initrd.availableKernelModules;
    forcedModules = config.boot.initrd.kernelModules;
  };
in {
  options.programs.zbm-rs = {
    timeout = lib.mkOption { type = lib.types.ints.between 0 300; default = 5; description = "Reserved autoboot timeout in seconds (autoboot is not implemented yet)."; };
    ui.showSnapshots = lib.mkOption { type = lib.types.bool; default = true; description = "Reserved snapshot visibility policy."; };
    ui.title = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; description = "Optional TUI heading."; };
    manager.restartLimit = lib.mkOption { type = lib.types.ints.between 0 8; default = 2; description = "Rapid failure count that enters recovery; 0 and 1 recover on the first failure."; };
    zfs.importPolicy = lib.mkOption { type = lib.types.enum [ "host-id" "read-only" ]; default = "host-id"; description = "Reserved pool import policy; discovery does not import pools."; };
    configurationLimit = lib.mkOption { type = lib.types.ints.between 1 512; default = 20; description = "Reserved NixOS generation limit."; };
    kernelArgs = lib.mkOption { type = lib.types.listOf lib.types.str; default = []; description = "Reserved arguments for the future target kernel."; };
    enable = lib.mkEnableOption "build a standalone zbm-rs EFI boot manager";
    profile = lib.mkOption { type = lib.types.enum [ "portable" "host-only" ]; default = "portable"; };
    hardwareManifest = lib.mkOption { type = lib.types.nullOr lib.types.path; default = null; };
  };
  config = lib.mkIf cfg.enable {
    system.build.zbm-rs-efi = image;
    environment.systemPackages = [ image.passthru.binary ];
  };
}
