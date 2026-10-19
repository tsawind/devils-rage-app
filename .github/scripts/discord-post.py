"""Posts a release to Discord: version, channel, what changed, download link.

usage: discord-post.py <tag> <channel>
needs: DISCORD_WEBHOOK_URL (repo secret; nothing is posted without it),
       GITHUB_REPOSITORY, a checkout with tags (fetch-depth: 0)

"What changed" = the commit messages since the previous release of the same kind
(dev since the last dev build, stable since the last stable), without the
Co-Authored-By / Claude-Session lines.
"""

import json
import os
import re
import subprocess
import sys
import urllib.request

tag, channel = sys.argv[1], sys.argv[2]
hook = os.environ.get("DISCORD_WEBHOOK_URL", "").strip()
repo = os.environ.get("GITHUB_REPOSITORY", "tsawind/devils-rage-app")
if not hook:
    print("no DISCORD_WEBHOOK_URL secret: not posting")
    sys.exit(0)


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True).stdout


def kind(t):
    return "dev" if "-dev" in t else "beta" if "-beta" in t else "stable"


git("fetch", "--tags", "--force", "--quiet")
tags = [t for t in git("tag", "--list", "v*", "--sort=-v:refname").split() if kind(t) == kind(tag)]
if tag in tags:
    i = tags.index(tag)
    previous = tags[i + 1] if i + 1 < len(tags) else None
else:
    previous = tags[0] if tags else None

head = tag if tag in git("tag", "--list", tag) else "HEAD"
span = f"{previous}..{head}" if previous else f"-15 {head}"
log = git("log", "--no-merges", "--format=%s%n%b%x1e", *span.split())

notes = []
for entry in log.split("\x1e"):
    lines = [l.rstrip() for l in entry.strip().splitlines()]
    lines = [l for l in lines if l and not re.match(r"(Co-Authored-By|Claude-Session|Signed-off-by):", l, re.I)]
    if not lines:
        continue
    notes.append(f"**{lines[0]}**")
    notes.extend(l if l.lstrip().startswith(("-", "•")) else f"  {l}" for l in lines[1:])
    notes.append("")

version = tag.lstrip("v")
labels = {"dev": "🧪 Dev build (testers)", "beta": "🔶 Beta", "stable": "✅ Stable release"}
colors = {"dev": 0x6A6A72, "beta": 0xE08A1E, "stable": 0xD6242C}
description = "\n".join(notes).strip() or "No change notes."
if len(description) > 3800:
    description = description[:3800].rsplit("\n", 1)[0] + "\n…"

payload = {
    "username": "Devil's Rage app",
    "embeds": [
        {
            "title": f"Devil's Rage {version}",
            "url": f"https://github.com/{repo}/releases/tag/{tag}",
            "description": description,
            "color": colors[channel],
            "footer": {
                "text": f"{labels[channel]} · installed apps on this channel update themselves"
            },
        }
    ],
}

req = urllib.request.Request(
    hook,
    data=json.dumps(payload).encode(),
    headers={"Content-Type": "application/json", "User-Agent": "devils-rage-app-ci"},
)
with urllib.request.urlopen(req) as resp:
    print("posted to Discord:", resp.status)
