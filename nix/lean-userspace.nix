# Default appliance userspace and optional comparison profiles. Keep upstream ZFS cryptography and disk logic.
{ pkgs }:
let
  # Preserve the caller's ZFS version and kernel-module attribute.
  mkZfs = package: (package.override { enablePython = false; }).overrideAttrs (old: {
    buildInputs = builtins.filter (package: (package.pname or "") != "curl") old.buildInputs;
    configureFlags = builtins.filter (flag: !(builtins.elem flag [ "--enable-pam" ])) old.configureFlags
      ++ [ "--disable-pam" "--disable-nls" ];
    # Detection is optional upstream. Pin it off even if a transitive input gains curl.
    postPatch = old.postPatch + ''
      substituteInPlace config/user-libfetch.m4 \
        --replace-fail 'if curl-config --protocols' 'if false && curl-config --protocols'
    '';
  });
in {
  inherit mkZfs;
  # Native crypto and local keys remain available; URL key fetching is omitted.
  zfs = mkZfs pkgs.zfs_2_4;
  # Keep modern systemd udev, but link only the shared-library code it uses.
  systemdUdev = pkgs.systemdMinimal.overrideAttrs (old: {
    mesonFlags = old.mesonFlags ++ [ "-Dlink-udev-shared=false" ];
  });
  # Standalone udev alternative, retaining kmod and storage helpers. No hwdb is
  # used by the image. This is the image default, not modern Gentoo's default.
  udev = pkgs.eudev.overrideAttrs (old: {
    configureFlags = old.configureFlags ++ [ "--disable-hwdb" ];
  });
}
