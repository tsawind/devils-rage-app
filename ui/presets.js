// Patch 69/70: sound & graphics presets shared by Client tuning and New launch group.
// Music always off; world (wormhole activations) and interface (alarms) never muted.
  // Suggested values. Music always off; world (wormhole activations) and interface (alarms) never muted.
const LOW = { textureQuality: 2, shaderQuality: 1, lodQuality: 1, shadowQuality: 0, reflectionQuality: 0, aoQuality: 0, postProcessingQuality: 0, volumetricQuality: 0, antiAliasing: 0, charTextureQuality: 2, dofEnabled: 0, charClothSimulation: 0, frameGeneration: false };
const QUIET_MUSIC = { eveampGain: 0, useCombatMusic: 0, suppressTurret: 1 };
window.DR_PRESETS = [
    { id: "main", name: "Main (the one you fly)", why: "Good-looking, every warning loud, no music.",
      values: { masterVolume: 0.8, worldVolume: 0.8, uiGain: 0.7, evevoiceGain: 0.9, ...QUIET_MUSIC,
                textureQuality: 1, shaderQuality: 2, lodQuality: 2, shadowQuality: 1, reflectionQuality: 1, aoQuality: 1, postProcessingQuality: 1, volumetricQuality: 1, antiAliasing: 1, charTextureQuality: 1, dofEnabled: 0, charClothSimulation: 0, frameGeneration: false },
      prefs: { bracketsAlwaysShowShipText: true } },
    { id: "alt", name: "Alt / multibox", why: "Light on the PC; you still hear activations and alarms, a bit quieter.",
      values: { masterVolume: 0.5, worldVolume: 0.7, uiGain: 0.6, evevoiceGain: 0.7, ...QUIET_MUSIC, ...LOW },
      prefs: { bracketsAlwaysShowShipText: true } },
    { id: "watch", name: "Hole watcher / scout", why: "Lightest graphics, world sound up so a wormhole activation is heard over everything.",
      values: { masterVolume: 0.7, worldVolume: 1.0, uiGain: 0.8, evevoiceGain: 0.9, ...QUIET_MUSIC, ...LOW },
      prefs: { bracketsAlwaysShowShipText: true } },
    { id: "background", name: "Background (PI, industry, market)", why: "Lightest graphics, quiet but not silent: alarms still come through.",
      values: { masterVolume: 0.35, worldVolume: 0.3, uiGain: 0.6, evevoiceGain: 0.6, ...QUIET_MUSIC, ...LOW },
      prefs: {} },
    { id: "redeem", name: "Daily redeem (lots of alts at once)", why: "Lowest everything and nearly silent: for logging a big batch in to claim the daily and log out. Smallest font, no crash-report uploads.",
      values: { masterVolume: 0.1, worldVolume: 0.2, uiGain: 0.4, evevoiceGain: 0.3, ...QUIET_MUSIC, ...LOW, brightness: 1, clientFontSize: 0 },
      prefs: { breakpadUpload: false } },
    { id: "scanner", name: "PI scanner (sharp planet, no lighting tricks)", why: "For the character that scans planets: sharp textures and icons so the app can follow the planet and find buildings; no shadows, reflections or haze, so a spot looks the same from every angle.",
      values: { ...QUIET_MUSIC, textureQuality: 0, shaderQuality: 3, lodQuality: 3, shadowQuality: 0, reflectionQuality: 0, aoQuality: 0, postProcessingQuality: 1, volumetricQuality: 0, antiAliasing: 2 },
      prefs: {} },
    { id: "sound", name: "Just kill the music", why: "Changes only music, combat music and turret noise; leaves everything else.",
      values: { ...QUIET_MUSIC }, prefs: {} },
  ];
