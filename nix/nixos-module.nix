{ self }:
{ config, lib, pkgs, ... }:
let
  cfg = config.programs.zbm-rs;
  image = self.lib.mkImage {
    inherit pkgs;
    loaderConfig = {
      ui = { timeout_secs = cfg.settings.ui.timeout; show_snapshots = cfg.settings.ui.showSnapshots; title = cfg.settings.ui.title; };
      manager.restart_limit = cfg.settings.manager.restartLimit;
      zfs.import_policy = cfg.settings.zfs.importPolicy;
      nixos.generation_limit = cfg.settings.nixos.generationLimit;
      kernel_args = cfg.settings.kernelArgs;
    };
    source = self;
    zfsSource = self.inputs.zfskit;
    kernelPackages = config.boot.kernelPackages;
    zfsUserspace = (import ./lean-userspace.nix { inherit pkgs; }).mkZfs config.boot.zfs.package;
    profile = cfg.image.profile;
    hardwareManifest = cfg.image.hardwareManifest;
    extraModules = config.boot.initrd.availableKernelModules;
    forcedModules = config.boot.initrd.kernelModules;
  };
in {
  imports = map (pair: lib.mkRenamedOptionModule
    ([ "programs" "zbm-rs" ] ++ builtins.elemAt pair 0)
    ([ "programs" "zbm-rs" ] ++ builtins.elemAt pair 1)) [
      [ [ "timeout" ] [ "settings" "ui" "timeout" ] ]
      [ [ "ui" "showSnapshots" ] [ "settings" "ui" "showSnapshots" ] ]
      [ [ "ui" "title" ] [ "settings" "ui" "title" ] ]
      [ [ "manager" "restartLimit" ] [ "settings" "manager" "restartLimit" ] ]
      [ [ "zfs" "importPolicy" ] [ "settings" "zfs" "importPolicy" ] ]
      [ [ "configurationLimit" ] [ "settings" "nixos" "generationLimit" ] ]
      [ [ "kernelArgs" ] [ "settings" "kernelArgs" ] ]
      [ [ "profile" ] [ "image" "profile" ] ]
      [ [ "hardwareManifest" ] [ "image" "hardwareManifest" ] ]
    ];
  options.programs.zbm-rs = {
    settings.ui.timeout = lib.mkOption { type = lib.types.ints.between 0 300; default = 5; description = "Reserved autoboot timeout in seconds (autoboot is not implemented yet)."; };
    settings.ui.showSnapshots = lib.mkOption { type = lib.types.bool; default = true; description = "Reserved snapshot visibility policy."; };
    settings.ui.title = lib.mkOption { type = lib.types.nullOr lib.types.str; default = null; description = "Optional TUI heading."; };
    settings.manager.restartLimit = lib.mkOption { type = lib.types.ints.between 0 8; default = 2; description = "Rapid failure count that enters recovery; 0 and 1 recover on the first failure."; };
    settings.zfs.importPolicy = lib.mkOption { type = lib.types.enum [ "host-id" "read-only" ]; default = "host-id"; description = "Policy for explicit selected-pool import; discovery never imports or forces import."; };
    settings.nixos.generationLimit = lib.mkOption { type = lib.types.ints.between 1 512; default = 20; description = "Maximum NixOS generations per root dataset."; };
    settings.kernelArgs = lib.mkOption { type = lib.types.listOf lib.types.str; default = []; description = "Additional target kernel arguments; overriding Bootspec init or whitespace/quoting is unsupported."; };
    enable = lib.mkEnableOption "build a standalone zbm-rs EFI boot manager";
    image.profile = lib.mkOption { type = lib.types.enum [ "portable" "host-only" ]; default = "portable"; };
    image.hardwareManifest = lib.mkOption { type = lib.types.nullOr lib.types.path; default = null; };
  };
  config = lib.mkIf cfg.enable {
    system.build.zbm-rs-efi = image;
    environment.systemPackages = [ image.passthru.binary ];
  };
}
