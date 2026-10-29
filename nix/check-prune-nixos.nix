{ pkgs }:
pkgs.runCommand "zbm-prune-nixos-test" { nativeBuildInputs = [ pkgs.uv pkgs.python3 ]; } ''
  cp ${./prune-nixos.py} prune-nixos.py
  cp ${./check-prune-nixos.py} check-prune-nixos.py
  UV_NO_CACHE=1 uv run --offline --no-project --no-managed-python --python ${pkgs.python3}/bin/python check-prune-nixos.py
  touch "$out"
''
