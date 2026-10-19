# Devil's Rage desktop app

A desktop window around the Devil's Rage mapper (https://devilsrage.duckdns.org), built with
[Tauri 2](https://v2.tauri.app). The server stays the Laravel mapper + SeAT; the app loads its
pages and adds native features the browser can't do (always-on-top scanner windows, clipboard
watching, client previews, hotkeys).

Plan and decisions: `claude/desktop-app-plan.md` in the Devils Rage VPS project.

## Status
- [x] Phase 0.1: one window that opens the mapper; login happens inside the app
- [ ] Phase 0.2: self-update (signed builds, GitHub Releases, stable / beta / dev)
- [ ] Phase 0.3: settings, developer panel, admin panel, server feature flags

## Getting the app (testers)
Every push to `main` builds Windows, macOS and Linux installers. Open the **Actions** tab,
pick the latest **Build** run, and download `installers-windows-latest` from **Artifacts**.
Run the `.exe` inside (Windows may warn that it is unsigned: More info → Run anyway).

Pushing a tag such as `v0.1.0` builds the same installers into a draft GitHub Release.

## Building it yourself (Windows)
1. Install [Rust](https://rustup.rs), [Node.js 22](https://nodejs.org) and the
   "Desktop development with C++" workload from Visual Studio Build Tools.
2. In this folder: `npm install`, then `npm run dev` to run it, or `npm run build` for an installer.

## CCP rules we follow
The app only reads EVE's chat/combat log files, the clipboard and ESI. It never sends input to
the EVE client and never reads game memory.
