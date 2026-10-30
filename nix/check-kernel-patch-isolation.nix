# Equivalent patches imported through different flake roots must retain the
# same store paths, so a UI/doc commit cannot rebuild the protected kernel.
{ pkgs }:
let
  copy = name: builtins.path {
    path = ./.;
    inherit name;
    filter = path: _:
      let relative = pkgs.lib.removePrefix (toString ./. + "/") path;
      in path == toString ./. || relative == "hibernation-tpm-patches.nix"
        || relative == "patches" || pkgs.lib.hasPrefix "patches/" relative;
  };
  paths = source: map (entry: toString entry.patch)
    (import (source + "/hibernation-tpm-patches.nix"));
  first = copy "kernel-patch-source-a";
  second = copy "kernel-patch-source-b";
in
assert first != second;
assert paths first == paths second;
assert paths first == paths ./.;
pkgs.runCommand "zbm-kernel-patch-isolation-check" {} ''touch $out''
