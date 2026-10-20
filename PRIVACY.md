# Privacy

The Devil's Rage app is a window around our corp's own sites: the mapper (devilsrage.duckdns.org) and SeAT (seatdevilsrage.duckdns.org). It has no accounts of its own, holds no EVE logins or passwords, and sends nothing to anyone else.

## What the app reads on your PC
- **EVE's Local chat logs** (Documents\EVE\logs\Chatlogs): only to see which system each of your characters is in.
- **The clipboard**: only to notice a probe scan you copied in EVE. Anything else you copy is ignored and never sent.
- **The title of the window in front** ("EVE - <character name>"): only to know which character a copied probe scan came from.

It never reads EVE's memory and never sends keys or clicks to EVE.

## What it sends, and where
| What | Where to | When | Turn it off |
|---|---|---|---|
| Your characters' current system | Our mapper, through the mapper page you're logged in to | When a character changes system | Settings → Location → Instant location |
| Probe scans you copy in EVE | Our mapper, the same way | When you copy one | Settings → Probe scans |
| A request for app settings (message of the day, feature switches); contains nothing about you | Our mapper | At start and every 30 minutes | Not switchable; it's needed to tell you about updates |
| An update check | GitHub (where the app is published) | A few seconds after start, then every 4 hours, or when you press Check for updates | Not switchable; installing an update always asks you first |

Pages you open in the app (mapper, SeAT, EVE's login page, Discord) work as they do in a browser, under those sites' own rules.

## Kept on your PC
The app's settings and its log file stay in `%APPDATA%\com.tsawind.devilsrage` and `%LOCALAPPDATA%\com.tsawind.devilsrage`. Nothing is uploaded unless you send the log yourself. Uninstalling the app (Windows Settings → Apps) removes it; delete those two folders to remove the settings and log as well.

## Questions
Open an issue at https://github.com/tsawind/devils-rage-app/issues or ask Tsawind in game.
