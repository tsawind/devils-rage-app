"""Works out the version, channel and release settings for a build.

Tag  v0.2.0          -> stable 0.2.0, draft release (Tsa publishes it by hand)
Tag  v0.2.0-beta.1   -> beta, published pre-release
Push to main         -> dev, version = next patch after the newest version so far,
                        e.g. 0.2.1-dev.37 (37 = the workflow run number)

Prints key=value lines for $GITHUB_OUTPUT.
"""

import json
import os
import re
import subprocess

CORE = re.compile(r"^v?(\d+)\.(\d+)\.(\d+)(?:-(.+))?$")


def core(version):
    m = CORE.match(version)
    return tuple(int(x) for x in m.groups()[:3]) if m else None


ref_type = os.environ.get("GITHUB_REF_TYPE", "")
ref_name = os.environ.get("GITHUB_REF_NAME", "")
run_number = os.environ.get("GITHUB_RUN_NUMBER", "0")

if ref_type == "tag":
    m = CORE.match(ref_name)
    if not m:
        raise SystemExit(f"tag {ref_name!r} is not like v1.2.3 or v1.2.3-beta.1")
    version = ref_name.lstrip("v")
    pre = m.group(4) or ""
    channel = "dev" if pre.startswith("dev") else "beta" if pre else "stable"
else:
    with open("src-tauri/tauri.conf.json") as f:
        newest = core(json.load(f)["version"])
    tags = subprocess.run(["git", "tag", "--list", "v*"], capture_output=True, text=True).stdout
    for tag in tags.split():
        if "-dev" in tag:
            continue  # dev builds don't move the base, or every push bumps the patch
        c = core(tag)
        if c and c > newest:
            newest = c
    major, minor, patch = newest
    version = f"{major}.{minor}.{patch + 1}-dev.{run_number}"
    channel = "dev"

print(f"version={version}")
print(f"tag=v{version}")
print(f"channel={channel}")
print(f"draft={'true' if channel == 'stable' else 'false'}")
print(f"prerelease={'false' if channel == 'stable' else 'true'}")
