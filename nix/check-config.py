"""Exercise Cargo config validation and input tracking. Run with uv run."""
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]

def build(path):
    env = os.environ.copy()
    env["ZBM_RS_CONFIG"] = str(path)
    result = subprocess.run(
        ["cargo", "build", "-p", "zbm-rs", "--locked", "--message-format=json"],
        cwd=ROOT, env=env, text=True, capture_output=True,
    )
    return result

with tempfile.TemporaryDirectory(prefix="zbm-config-check-") as directory:
    path = Path(directory) / "config.json"
    fixture = json.loads((ROOT / "config/test.json").read_text())
    path.write_text(json.dumps(fixture))
    valid = build(path)
    assert valid.returncode == 0, valid.stderr
    messages = [json.loads(line) for line in valid.stdout.splitlines() if line.startswith("{")]
    script = next(m for m in messages if m.get("reason") == "build-script-executed" and "zbm-rs@" in m["package_id"])
    directives = (Path(script["out_dir"]).parent / "output").read_text()
    assert "cargo::rerun-if-env-changed=ZBM_RS_CONFIG" in directives
    assert f"cargo::rerun-if-changed={path}" in directives
    assert not (Path(script["out_dir"]) / "config.rs").exists(), "Unexpected generated Rust config"
    fixture["nixos"]["generation_limit"] = 0
    path.write_text(json.dumps(fixture))
    invalid = build(path)
    assert invalid.returncode != 0, "Changed invalid JSON did not invalidate Cargo build"
    assert "nixos.generation_limit" in invalid.stderr, invalid.stderr
    path.write_text('{"zfs":{"import_policy":"unsafe"}}')
    invalid = build(path)
    assert invalid.returncode != 0 and "unknown variant" in invalid.stderr, invalid.stderr
    path.write_text("{}")
    assert build(path).returncode == 0, "Defaults should validate"
    other = Path(directory) / "other.json"
    other.write_text('{"manager":{"restart_limit":99}}')
    invalid = build(other)
    assert invalid.returncode != 0 and "manager.restart_limit" in invalid.stderr, invalid.stderr

result = subprocess.run(["cargo", "build", "-p", "zbm-rs", "--locked"], cwd=ROOT,
    env={key: value for key, value in os.environ.items() if key != "ZBM_RS_CONFIG"}, capture_output=True, text=True)
assert result.returncode == 0, result.stderr
print("Config checks passed: typed JSON, defaults, validation, env/file input tracking, no Rust codegen")
