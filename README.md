# Devil's Rage desktop app

A desktop window around the Devil's Rage mapper (https://devilsrage.duckdns.org), built with
[Tauri 2](https://v2.tauri.app). The server stays the Laravel mapper + SeAT; the app loads its
pages and adds native features the browser can't do (always-on-top scanner windows, clipboard
watching, client previews, hotkeys).

Plan and decisions: `claude/desktop-app-plan.md` in the Devils Rage VPS project.

## Status
- [x] Phase 0.1: one window that opens the mapper; login happens inside the app
- [x] Phase 0.2: self-update (signed builds, GitHub Releases, stable / beta / dev)
- [ ] Phase 0.3: settings, developer panel, admin panel, server feature flags

## Getting the app
Download the `-setup.exe` from the **Releases** page (Windows may warn that it is unsigned:
More info → Run anyway). After that the app updates itself.

## Releases and update channels
| Do this | You get | Who receives it |
|---|---|---|
| Push to `main` | `0.x.y-dev.N` pre-release | people on a dev build |
| Push tag `v0.2.0-beta.1` | beta pre-release | people on a beta build |
| Push tag `v0.2.0`, then **Publish** the draft on GitHub | stable release | stable and beta users |

Easiest way to release without git: **Actions → Build → Run workflow**, type the version
(`0.2.0`, or `0.2.0-beta.1` for a beta) and press the green button. A stable build lands as a
**draft** on the Releases page; open it and click **Publish release** to ship it.

The app follows the channel of the build that was installed. To switch by hand, put
`{"channel": "beta"}` (or `stable` / `dev`) in `%APPDATA%\com.tsawind.devilsrage\settings.json`.
Channel manifests live on the `updates` branch; don't edit it by hand.

Every published build (dev, beta, stable) is posted to Discord with its change notes, when the
repo secret `DISCORD_WEBHOOK_URL` is set. Posts are silent (no sound or popup). With the optional
secret `DISCORD_WEBHOOK_URL_TESTING`, dev and beta builds go to that channel instead and only stable
releases go to the main one. The notes are the commit messages since the previous
build of the same kind, so commit messages are written as plain-English change notes.

Builds are signed with the key in the repo secrets `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Tsa keeps the backup. Lose it and installed apps can't update.

## Building it yourself (Windows)
1. Install [Rust](https://rustup.rs), [Node.js 22](https://nodejs.org) and the
   "Desktop development with C++" workload from Visual Studio Build Tools.
2. In this folder: `npm install`, then `npm run dev` to run it, or `npm run build` for an installer.

## CCP rules we follow
The app only reads EVE's chat/combat log files, the clipboard and ESI. It never sends input to
the EVE client and never reads game memory.
