{ self, pkgs }:
let
  probe = pkgs.rustPlatform.buildRustPackage {
    pname = "zbm-resume-device-probe";
    version = "0.1.0";
    src = pkgs.lib.cleanSourceWith {
      src = self;
      filter = path: _:
        let relative = pkgs.lib.removePrefix (toString self + "/") path;
        in path == toString self || pkgs.lib.any
          (name: relative == name || pkgs.lib.hasPrefix (name + "/") relative)
          [ "Cargo.toml" "Cargo.lock" "crates" "xtask" ];
    };
    cargoLock.lockFile = ../Cargo.lock;
    nativeBuildInputs = [ pkgs.pkg-config ];
    buildInputs = [ pkgs.openssl ];
    cargoBuildFlags = [ "-p" "zbm-core" "--example" "resume-device-probe" ];
    doCheck = false;
    installPhase = ''
      install -Dm755 target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/release/examples/resume-device-probe $out/bin/resume-device-probe
    '';
  };
in
pkgs.testers.runNixOSTest {
  name = "zbm-luks2-resume-device";
  globalTimeout = 300;
  nodes.machine = { ... }: {
    virtualisation.memorySize = 2048;
    virtualisation.emptyDiskImages = [ 768 ];
    environment.systemPackages = [ probe pkgs.cryptsetup pkgs.lvm2 pkgs.gptfdisk pkgs.python3 ];
  };
  testScript = ''
    machine.start()
    machine.wait_for_unit("multi-user.target")
    machine.succeed("${pkgs.python3}/bin/python3 ${./check-luks2-resume.py}", timeout=180)
  '';
}
