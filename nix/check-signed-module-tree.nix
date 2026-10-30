{ self, pkgs, kernel ? pkgs.linuxPackages.kernel }:
let
  # A separate derivation gives the fixture the real read-only store-root mode.
  source = pkgs.runCommand "module-signing-read-only-fixture" {} ''
    module=$(find ${kernel.modules}/lib/modules/${kernel.modDirVersion}/kernel \
      -type f -name '*.ko*' -print -quit)
    test -n "$module"
    mkdir -p "$out/lib/modules/${kernel.modDirVersion}/kernel"
    cp "$module" "$out/lib/modules/${kernel.modDirVersion}/kernel/"
  '';
in pkgs.runCommand "signed-module-tree-check" {
  nativeBuildInputs = [ pkgs.uv pkgs.python3 pkgs.openssl pkgs.xz pkgs.zstd
    pkgs.stdenv.cc pkgs.pkg-config pkgs.sbsigntool ];
  buildInputs = [ pkgs.openssl ];
} ''
  export UV_CACHE_DIR="$TMPDIR/uv-cache"
  test ! -w ${source}
  find ${source} -type f -exec sha256sum '{}' + > original.sha256
  umask 077
  openssl req -x509 -newkey rsa:2048 -nodes -sha256 -days 1 \
    -subj /CN=Disposable-Module-Signing-Test \
    -keyout "$TMPDIR/key.pem" -out "$TMPDIR/cert.pem" >/dev/null 2>&1
  # Ordinary kernels need not build this host tool. Compile its real source
  # without enabling module signing or rebuilding the fixture kernel.
  cc ${kernel.dev}/lib/modules/${kernel.modDirVersion}/source/scripts/sign-file.c \
    -o "$TMPDIR/sign-file" $(pkg-config --cflags --libs libcrypto)
  args=(
    ${source} "$TMPDIR/signed"
    --key "$TMPDIR/key.pem" --cert "$TMPDIR/cert.pem"
    --sign-file "$TMPDIR/sign-file"
    --kernel ${kernel}
  )
  uv run --offline --no-project --python ${pkgs.python3}/bin/python \
    ${self}/nix/sign-module-tree.py "''${args[@]}"
  test -w "$TMPDIR/signed"
  uv run --offline --no-project --python ${pkgs.python3}/bin/python - <<'PY'
  import json, os
  from pathlib import Path
  manifest = json.loads((Path(os.environ["TMPDIR"]) / "signed/zbm-module-manifest.json").read_text())
  assert manifest["version"] == 1 and manifest["signed_modules"] == 1
  assert manifest["kernel_store_path"] == "${kernel}"
  assert manifest["module_version"] == "${kernel.modDirVersion}"
  PY
  sha256sum -c original.sha256
  if uv run --offline --no-project --python ${pkgs.python3}/bin/python \
    ${self}/nix/sign-module-tree.py "''${args[@]}" > reject.log 2>&1; then
    echo 'existing signed tree was overwritten' >&2
    exit 1
  fi
  grep -q 'fresh output outside the store' reject.log
  # Publicly pre-signed kernels are signed again by the installation hook.
  # Both certificate tables must normalize to the exact approved kernel bytes.
  sbsign --key "$TMPDIR/key.pem" --cert "$TMPDIR/cert.pem" \
    --output "$TMPDIR/kernel-once" ${kernel}/bzImage
  sbsign --key "$TMPDIR/key.pem" --cert "$TMPDIR/cert.pem" \
    --output "$TMPDIR/kernel-twice" "$TMPDIR/kernel-once"
  sbverify --cert "$TMPDIR/cert.pem" "$TMPDIR/kernel-twice"
  export PYTHONDONTWRITEBYTECODE=1
  uv run --offline --no-project --python ${pkgs.python3}/bin/python - <<'PY'
  import importlib.util, os
  from pathlib import Path
  spec = importlib.util.spec_from_file_location("protected", "${self}/nix/protected-hibernation.py")
  validator = importlib.util.module_from_spec(spec)
  spec.loader.exec_module(validator)
  temporary = Path(os.environ["TMPDIR"])
  approved = Path("${kernel}/bzImage")
  for name in ("kernel-once", "kernel-twice"):
      validator.validate_kernel(temporary / name, approved)
  changed = bytearray((temporary / "kernel-twice").read_bytes())
  changed[4096] ^= 1
  (temporary / "kernel-changed").write_bytes(changed)
  try:
      validator.validate_kernel(temporary / "kernel-changed", approved)
  except ValueError:
      pass
  else:
      raise AssertionError("A changed signed kernel payload was approved")
  PY
  rm "$TMPDIR/key.pem"
  touch "$out"
''
