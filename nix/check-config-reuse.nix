{ pkgs, mkImage }:
let
  base = mkImage { loaderConfig = {}; };
  changed = mkImage { loaderConfig = {
    ui = { title = "Alternate menu"; show_snapshots = false; };
    boot = { mode = "countdown"; timeout_secs = 17; };
    nixos.generation_limit = 7;
    nixos.preferred_specialisation = "niri";
    zfs.import_policy = "read-only";
    security.tpm.policy = "optional";
  }; };
  diagnostics = mkImage { loaderConfig.diagnostics.esp_partuuid = "ed5d4b72-0aac-48d3-8f89-43362d952ca0"; };
in
assert base.binary.drvPath == changed.binary.drvPath;
assert base.configJson.drvPath != changed.configJson.drvPath;
assert base.runtime.drvPath != changed.runtime.drvPath;
assert base.binary.drvPath != diagnostics.binary.drvPath;
pkgs.runCommand "zbm-config-binary-reuse" { nativeBuildInputs = [ pkgs.jq ]; } ''
  jq -e '.boot.timeout_secs == 5' ${base.configJson}
  jq -e '.ui.title == "Alternate menu" and .boot.timeout_secs == 17 and .security.tpm.policy == "optional"' ${changed.configJson}
  touch $out
''
