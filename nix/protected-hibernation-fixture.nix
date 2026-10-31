# Runtime test inputs contain public artifacts and a disposable secret *path*.
# Signing keys are never parameters or Nix inputs. The runtime driver signs the
# final copies outside the store after discovering its disposable pool GUID.
{ parameters }:
let
  input = builtins.fromJSON (builtins.readFile parameters);
  self = builtins.getFlake ("path:" + input.source);
  kernelSource = if input ? kernelSource then builtins.getFlake ("path:" + input.kernelSource) else self;
  system = "x86_64-linux";
  nixpkgs = self.inputs.nixpkgs;
  pkgs = import nixpkgs { inherit system; config.allowUnfree = true; };
  publicCertificate = path: name:
    let text = builtins.readFile path;
    in assert pkgs.lib.assertMsg (builtins.stringLength text <= 65536
      && pkgs.lib.hasInfix "BEGIN CERTIFICATE" text && !(pkgs.lib.hasInfix "PRIVATE KEY" text))
      "The runtime fixture accepts public certificates only";
      builtins.path { inherit path name; };
  certificate = publicCertificate input.certificate "vm-signing.pem";
  imaCertificate = publicCertificate input.imaCertificate "vm-ima.pem";
  signedKernel = builtins.path { path = input.signedKernel; name = "signed-kernel-v3.efi"; };
  signedModules = builtins.path { path = input.signedModules; name = "signed-modules-v3"; };
  kernelPackages = import "${kernelSource}/nix/security-kernel.nix" {
    inherit pkgs;
    kernelPackages = pkgs.linuxPackages_latest;
    trustedCertificates = [ certificate ];
    hibernationTpmSignature = true;
  };
  native = import ./check-encrypted-hibernation.nix {
    inherit self pkgs nixpkgs system kernelPackages signedKernel signedModules;
    credentialPoolGuid = input.poolGuid;
    secretSource = input.secretSource;
    enforcedTarget = true;
    targetSigning = {
      # The guest cannot invoke a signer. Actual disposable keys are available
      # only to the parent runtime publisher, outside Nix and the guest.
      kernelKey = "/run/fixture-signing-unavailable";
      imaKey = "/run/fixture-signing-unavailable";
      authorityKey = "/run/fixture-signing-unavailable";
      kernelCertificate = toString certificate;
      imaCertificate = toString imaCertificate;
      authorityCertificate = toString certificate;
    };
  };
  target = native.nativeTarget;
  closure = pkgs.closureInfo { rootPaths = [ target ]; };
  root = pkgs.runCommand "protected-hibernation-root-fixture" {
    nativeBuildInputs = [ pkgs.gnutar ];
  } ''
    mkdir -p root/nix/var/nix/profiles $out
    ln -s ${target} root/nix/var/nix/profiles/system-1-link
    ln -s system-1-link root/nix/var/nix/profiles/system
    tar -cf $out/root.tar -C root .
    sed 's,^/,,' ${closure}/store-paths > store-paths
    tar -rf $out/root.tar -C / -T store-paths
    printf '%s\n' ${target} > $out/target
    cp ${native.nativeTargetConfig.system.build.zbmProtectedHibernationContract} $out/contract.json
  '';
  loader = self.lib.mkImage {
    inherit pkgs kernelPackages imaCertificate;
    kernelPolicy = "validate";
    profile = "host-only";
    extraModules = [ "virtio_pci" "virtio_blk" "atkbd" "i8042" ];
    hostId = "00bab10c";
    targetAuthorities = [ certificate ];
    testProfile = true;
    forcedModules = [ "virtio_net" ];
    loaderConfig = {
      boot.mode = "menu";
      zfs.import_policy = "read-only";
      resume = native.nativeTargetConfig.boot.loader.zbm-rs.hibernation.resumeConfig;
      security = {
        mode = "enforce";
        require_firmware_secure_boot = true;
        tpm.policy = "optional";
      };
    };
  };
  bundle = pkgs.writeText "protected-hibernation-runtime-inputs.json" (builtins.toJSON {
    inherit root loader target;
    config = loader.configJson;
    binary = "${loader.binary}/bin/zbm-rs";
    sign_file = "${kernelPackages.kernel.dev}/lib/modules/${kernelPackages.kernel.modDirVersion}/build/scripts/sign-file";
    evmctl = "${pkgs.ima-evm-utils}/bin/evmctl";
    ukify = "${pkgs.systemdUkify}/bin/ukify";
  });
in { inherit root loader target bundle; }
