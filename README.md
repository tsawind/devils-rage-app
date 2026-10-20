# Devil's Rage desktop app

A desktop window around the Devil's Rage mapper (https://devilsrage.duckdns.org), built with
[Tauri 2](https://v2.tauri.app). The server stays the Laravel mapper + SeAT; the app loads its
pages and adds native features the browser can't do (always-on-top scanner windows, clipboard
watching, client previews, hotkeys).

Plan and decisions: `claude/desktop-app-plan.md` in the Devils Rage VPS project.

## Status
- [x] Phase 0: window around the mapper and SeAT, self-update (stable / beta / dev), settings, developer and admin panels
- [x] Phase 1 so far: scanner pop-out on top of EVE, instant location from EVE's chat logs, probe scans pasted from the clipboard, the jump prompt over EVE
- [ ] Next: your accounts overview, PI, client previews, hotkeys, tray

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

## Privacy
What the app reads, what it sends and how to switch it off: [PRIVACY.md](PRIVACY.md). The installer shows the
same summary before installing.

## Code signing policy
Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by
[SignPath Foundation](https://signpath.org).

- Only release builds made by this repository's GitHub Actions from this repository's source are signed.
- Every signing request is approved by hand.
- Team: Tsawind — author, reviewer and approver. Changes from anyone outside the team are reviewed by Tsawind
  before they are merged.
- Privacy policy: [PRIVACY.md](PRIVACY.md).

Signing is being set up; until it is, Windows may show an "unknown publisher" warning (More info → Run anyway).

## License
[MIT](LICENSE). Third-party code and its licenses: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
