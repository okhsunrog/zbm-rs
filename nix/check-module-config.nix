{ self, nixpkgs, pkgs, system, binary }:
let
  json = overrides: (nixpkgs.lib.nixosSystem {
    inherit system pkgs;
    modules = [ self.nixosModules.default {
      programs.zbm-rs = { enable = true; } // overrides;
    } ];
  }).config.system.build.zbm-rs-efi.passthru.configJson;
  defaults = json {};
  optionalTpm = json { settings.security.tpm.policy = "optional"; };
  requiredTpm = json { settings.security.tpm.policy = "required"; };
  minimum = json { settings = {
    ui.timeout = 0;
    manager.restartLimit = 0;
    zfs.importPolicy = "read-only";
    nixos.generationLimit = 1;
  }; };
  maximum = json { settings = {
    ui.timeout = 300;
    manager.restartLimit = 8;
    nixos.generationLimit = 512;
  }; };
  legacy = json {
    timeout = 0;
    manager.restartLimit = 0;
    zfs.importPolicy = "read-only";
    configurationLimit = 1;
  };
in pkgs.runCommand "zbm-module-config-parity" { nativeBuildInputs = [ pkgs.jq ]; } ''
  mkdir -p $out
  ${binary}/bin/zbm-rs --validate-config ${defaults} > $out/nix-defaults.json
  ${binary}/bin/zbm-rs --validate-config ${pkgs.writeText "rust-defaults.json" "{}"} > $out/rust-defaults.json
  cmp $out/nix-defaults.json $out/rust-defaults.json
  ${binary}/bin/zbm-rs --validate-config ${minimum} > $out/minimum.json
  ${binary}/bin/zbm-rs --validate-config ${maximum} > $out/maximum.json
  ${binary}/bin/zbm-rs --validate-config ${legacy} > $out/legacy.json
  cmp $out/minimum.json $out/legacy.json
  ${binary}/bin/zbm-rs --validate-config ${optionalTpm} > $out/tpm-optional.json
  ${binary}/bin/zbm-rs --validate-config ${requiredTpm} > $out/tpm-required.json
  jq -e '.security.tpm == {"policy":"optional"}' $out/tpm-optional.json
  jq -e '.security.tpm == {"policy":"required"}' $out/tpm-required.json
  jq -e '.ui.timeout_secs == 0 and .manager.restart_limit == 0 and .nixos.generation_limit == 1 and .zfs.import_policy == "read-only"' $out/minimum.json
  jq -e '.ui.timeout_secs == 300 and .manager.restart_limit == 8 and .nixos.generation_limit == 512' $out/maximum.json
''
