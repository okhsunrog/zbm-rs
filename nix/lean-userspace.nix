# Experimental appliance profiles. Keep upstream ZFS cryptography and disk logic.
{ pkgs }:
{
  # Gentoo's minimal mode already maps to enablePython=false. Additionally skip
  # optional URL fetching: this profile cannot use HTTPS encryption keylocations.
  zfs = (pkgs.zfs_2_4.override { enablePython = false; }).overrideAttrs (old: {
    buildInputs = builtins.filter (package: (package.pname or "") != "curl") old.buildInputs;
    configureFlags = builtins.filter (flag: !(builtins.elem flag [ "--enable-pam" ])) old.configureFlags
      ++ [ "--disable-pam" "--disable-nls" ];
    # Detection is optional upstream. Pin it off even if a transitive input gains curl.
    postPatch = old.postPatch + ''
      substituteInPlace config/user-libfetch.m4 \
        --replace-fail 'if curl-config --protocols' 'if false && curl-config --protocols'
    '';
  });
  # Keep modern systemd udev, but link only the shared-library code it uses.
  systemdUdev = pkgs.systemdMinimal.overrideAttrs (old: {
    mesonFlags = old.mesonFlags ++ [ "-Dlink-udev-shared=false" ];
  });
  # Standalone udev alternative, retaining kmod and storage helpers. No hwdb is
  # used by the image. This is an experiment, not a claim about modern Gentoo defaults.
  udev = pkgs.eudev.overrideAttrs (old: {
    configureFlags = old.configureFlags ++ [ "--disable-hwdb" ];
  });
}
