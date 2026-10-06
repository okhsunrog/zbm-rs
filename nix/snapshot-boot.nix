# Compatibility shim for existing imports. Clone boot is detected from initrd;
# this legacy option no longer changes Bootspec or gates any ZFS operation.
{ lib, ... }: {
  options.programs.zbm-rs.snapshotBoot.enable = lib.mkOption {
    type = lib.types.bool;
    default = false;
    description = "Deprecated compatibility option; initrd support is detected automatically.";
  };
}
