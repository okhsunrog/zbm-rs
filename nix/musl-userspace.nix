# Cross-build the complete boot userspace while using native build tools.
{ pkgs }:
pkgs.pkgsCross.musl64.extend (final: prev:
let
  withoutSqlite = inputs: builtins.filter (p: (p.pname or "") != "sqlite") inputs;
  bootUtilLinux = package: (package.override {
    withLastlog = false;
    systemdSupport = false;
    pamSupport = false;
  }).overrideAttrs (old: {
    buildInputs = withoutSqlite old.buildInputs;
  });
in if prev.stdenv.hostPlatform.isMusl then {
  # musl omits the RPC database API. libtirpc 1.3.7 provides it only when
  # explicitly enabled; nfs-utils needs getrpcbynumber during configuration.
  libtirpc = prev.libtirpc.overrideAttrs (old: {
    configureFlags = old.configureFlags ++ [ "--enable-rpcdb" ];
  });
  # All libudev consumers use the same standalone implementation as the image.
  udev = final.eudev;
  # vdev_id needs udevadm, not systemd. The lean image also disables ZFS PAM.
  zfs_2_4 = (prev.zfs_2_4.override { systemdMinimal = final.eudev; }).overrideAttrs (old: {
    buildInputs = builtins.filter (p: (p.pname or "") != "linux-pam") old.buildInputs;
  });
  # SQLite is only needed for lastlog2, not libblkid/libuuid/libmount.
  util-linux = bootUtilLinux prev.util-linux;
  util-linuxMinimal = bootUtilLinux prev.util-linuxMinimal;
  # ZFS references exportfs; NFS client tracking daemons are not boot tools.
  nfs-utils = (prev.nfs-utils.override { enableLdap = false; enableSystemd = false; }).overrideAttrs (old: {
    # nfs-utils 3.1.1 inserted limits.h in the old patch's context. Preserve
    # the required musl libgen.h fix with refreshed context and all other patches.
    patches = let
      isIncludesPatch = patch: pkgs.lib.hasSuffix "-musl-includes.patch" (toString patch);
    in assert builtins.length (builtins.filter isIncludesPatch old.patches) == 1;
      (map (patch: if isIncludesPatch patch then ./patches/nfs-utils-musl-includes.patch else patch) old.patches)
      ++ [ ./patches/nfs-utils-musl-thread-format.patch ];
    buildInputs = withoutSqlite old.buildInputs;
    configureFlags = old.configureFlags ++ [ "--disable-nfsdcld" "--disable-nfsdcltrack" ];
    # Upstream has no configure switch for the SQLite-backed fsidd daemon.
    # Keep libreexport (used by exportfs), but omit this unused server daemon.
    postPatch = old.postPatch + ''
      substituteInPlace support/reexport/Makefile.in \
        --replace-fail 'sbin_PROGRAMS = fsidd$(EXEEXT)' 'sbin_PROGRAMS ='
    '';
  });
} else {})
