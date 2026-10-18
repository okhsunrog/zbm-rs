"""Check runtime validation, binary reuse and compiled policy tracking. Run with uv run."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]

def build(path=None, policy=None):
    env = os.environ.copy()
    env.pop("ZBM_RS_CONFIG", None)
    env.pop("ZBM_RS_BUILD_POLICY", None)
    if path is not None:
        env["ZBM_RS_CONFIG"] = str(path)
    if policy is not None:
        env["ZBM_RS_BUILD_POLICY"] = json.dumps(policy)
    return subprocess.run(
        ["cargo", "build", "-p", "zbm-rs", "--locked", "--message-format=json"],
        cwd=ROOT, env=env, text=True, capture_output=True,
    )


def artifact(result):
    assert result.returncode == 0, result.stderr
    messages = [json.loads(line) for line in result.stdout.splitlines() if line.startswith("{")]
    return next(m for m in messages if m.get("reason") == "compiler-artifact"
                and m["target"]["name"] == "zbm-rs" and m.get("executable"))


def run(binary, *args, config=None):
    env = os.environ.copy()
    env.pop("ZBM_RS_CONFIG", None)
    if config is not None:
        env["ZBM_RS_CONFIG"] = str(config)
    return subprocess.run([str(binary), *args], env=env, text=True, capture_output=True)


baseline = artifact(build())
binary = Path(baseline["executable"])
digest = hashlib.sha256(binary.read_bytes()).hexdigest()

with tempfile.TemporaryDirectory(prefix="zbm-config-check-") as directory:
    path = Path(directory) / "config.json"
    fixture = json.loads((ROOT / "config/test.json").read_text())
    path.write_text(json.dumps(fixture))
    assert artifact(build(path))["fresh"], "Runtime config path rebuilt the executable"
    valid = run(binary, "--validate-config", str(path))
    assert valid.returncode == 0, valid.stderr
    preview = run(binary, "--print-config", config=path)
    assert json.loads(preview.stdout)["ui"]["title"] == fixture["ui"]["title"]
    fixture["ui"]["title"] = "Changed without recompilation"
    path.write_text(json.dumps(fixture))
    assert artifact(build(path))["fresh"], "Runtime config content rebuilt the executable"
    assert json.loads(run(binary, "--print-config", config=path).stdout)["ui"]["title"] == fixture["ui"]["title"]
    fixture["nixos"]["generation_limit"] = 0
    path.write_text(json.dumps(fixture))
    assert artifact(build(path))["fresh"], "Runtime JSON unexpectedly participates in compilation"
    invalid = run(binary, "--validate-config", str(path))
    assert invalid.returncode != 0, "Invalid image JSON passed validation"
    assert "nixos.generation_limit" in invalid.stderr, invalid.stderr
    path.write_text('{"zfs":{"import_policy":"unsafe"}}')
    invalid = run(binary, "--validate-config", str(path))
    assert invalid.returncode != 0 and "unknown variant" in invalid.stderr, invalid.stderr
    path.write_text("{}")
    assert run(binary, "--validate-config", str(path)).returncode == 0, "Defaults should validate"
    missing = Path(directory) / "missing.json"
    assert artifact(build(missing))["fresh"], "A missing runtime file must not affect compilation"
    assert run(binary, "--print-config", config=missing).returncode != 0
    assert hashlib.sha256(binary.read_bytes()).hexdigest() == digest

defaults = run(binary, "--validate-config", str(ROOT / "config/default.json"))
assert defaults.returncode == 0, defaults.stderr
assert json.loads(defaults.stdout) == json.loads(run(binary, "--print-config").stdout)

try:
    policy = {"security_mode": "enforce"}
    assert not artifact(build(policy=policy))["fresh"], "Compiled security policy change was ignored"
    policy["diagnostic_esp_partuuid"] = "ed5d4b72-0aac-48d3-8f89-43362d952ca0"
    assert not artifact(build(policy=policy))["fresh"], "Compiled ESP policy change was ignored"
    invalid = build(policy={"security_mode": "unsafe"})
    assert invalid.returncode != 0 and "unknown variant" in invalid.stderr, invalid.stderr
    invalid = build(policy={"diagnostic_esp_partuuid": "../../sda"})
    assert invalid.returncode != 0 and "GPT partition UUID" in invalid.stderr, invalid.stderr
    invalid = build(policy={"ui": {"timeout_secs": 10}})
    assert invalid.returncode != 0 and "unknown field" in invalid.stderr, invalid.stderr
finally:
    artifact(build())

print("Config checks passed: runtime validation, unchanged binary for JSON edits, tracked security/ESP build policy")
