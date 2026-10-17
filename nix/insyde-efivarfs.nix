# Build the separately maintained DMI-scoped recovery module for the loader ABI.
# sourceDirectory contains only public kernel-module source, never DKMS keys.
{ pkgs, kernelPackages, sourceDirectory }:
let
  kernel = kernelPackages.kernel;
  files = [ "Kbuild" "recovery.rs" "model.rs" "bridge.c" "bridge.h" ];
  source = pkgs.runCommand "insyde-efivarfs-source" {} ''
    mkdir -p $out
    ${pkgs.lib.concatMapStringsSep "\n" (file: let
      input = builtins.path { path = sourceDirectory + "/${file}"; name = "insyde-${file}"; };
    in "cp ${pkgs.lib.escapeShellArg "${input}"} $out/${file}") files}
  '';
in assert pkgs.lib.assertMsg (pkgs.lib.versionAtLeast kernel.version "7.2")
  "The maintained recovery source requires Linux >= 7.2 Rust module APIs; select a matching loader kernel";
pkgs.stdenv.mkDerivation {
  pname = "zbm-insyde-efivarfs";
  version = "0.1.0";
  src = source;
  nativeBuildInputs = kernel.moduleBuildDependencies;
  hardeningDisable = [ "pic" "format" ];
  buildPhase = ''
    runHook preBuild
    grep -qx CONFIG_RUST=y ${kernel.configfile} || {
      echo 'The Framework recovery module requires a Rust-enabled loader kernel' >&2
      exit 1
    }
    make -C ${kernel.dev}/lib/modules/${kernel.modDirVersion}/build M="$PWD" modules
    runHook postBuild
  '';
  installPhase = ''
    install -Dm444 insyde_efivarfs.ko $out/lib/modules/${kernel.modDirVersion}/extra/insyde_efivarfs.ko
  '';
  meta.license = pkgs.lib.licenses.gpl2Only;
}
