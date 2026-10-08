# Signed-policy NvPCR support requires the v262 provider API. This is a separate
# loader dependency; host systemd and the installed OS are never replaced.
{ pkgs }:
(pkgs.systemdMinimal.override { withTpm2Tss = true; withOpenSSL = true; withBootloader = true; withEfi = true; }).overrideAttrs (old: {
  version = "262";
  src = pkgs.fetchzip {
    url = "https://github.com/systemd/systemd/archive/refs/tags/v262.tar.gz";
    hash = "sha256-oGzFW2dD8abLXBwDczr1hvl712s03CHTUqOz6uPfhmQ=";
  };
  # The distribution timezone patch targets unrelated firstboot/timedated code
  # and does not apply to v262. Keep the Nix path lookup patches for our helpers.
  patches = builtins.filter (patch: !(builtins.elem (builtins.baseNameOf (toString patch)) [
    "0002-Change-usr-share-zoneinfo-to-etc-zoneinfo.patch"
    "0006-timesyncd-disable-NSCD-when-DNSSEC-validation-is-dis.patch"
  ])) old.patches;
})
