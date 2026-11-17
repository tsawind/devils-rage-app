// Client tuning tiers (17 Nov 2026, Tsa): four presets shared by Client tuning and New launch group.
//
// The app writes what EVE keeps in each profile's core_public__.yaml (graphics, the five sound
// sliders, music, quieter weapons) and prefs.ini. The Effects tick boxes and EVE's Advanced audio
// sliders (jump activation, ship ambience, turret volume…) are kept per account in EVE's own
// binary files, which the app doesn't touch: each preset lists them under `manual` to set once
// in EVE (they stay set for that account).
//
// Graphics numbers as EVE stores them: texture & character texture 0 High · 1 Medium · 2 Low;
// shader & LOD 3 High · 2 Medium · 1 Low; anti-aliasing 3 High · 2 Medium · 1 Low · 0 Disabled;
// post-processing 2 High · 1 Medium · 0 Low; shadows & ambient occlusion 2 High · 0 Disabled;
// reflections 3 High · 0 Disabled; volumetrics 2 High · 1 Medium · 0 Low.

// Same sound on every tier: music off, jump activations and warnings heard, the rest at half.
const SOUND = { masterVolume: 0.5, worldVolume: 0.5, uiGain: 0.5, evevoiceGain: 0.5, eveampGain: 0, useCombatMusic: 0, suppressTurret: 1 };
const LOWEST = { antiAliasing: 0, postProcessingQuality: 0, shaderQuality: 1, textureQuality: 2, lodQuality: 1, shadowQuality: 0, aoQuality: 0, volumetricQuality: 0, reflectionQuality: 0, charTextureQuality: 2, dofEnabled: 0, charClothSimulation: 0, frameGeneration: false };

const AUDIO_HAND = "Audio → Advanced Audio Settings on: Jump Activation full; Ship Effects 25%; Ship Ambience 0; Turret Volume 0; Atmosphere, Secondary Interfaces, UI Click and Warning Sounds at half.";
const effects = (on) => {
  const all = ["Turrets & Launchers", "Effects", "Missile Effects", "Explosions", "Drone Models", "Trails", "GPU Particles", "Asteroid Environments", "Inspace Skinning Effect", "Depth Of Field"];
  const ticked = all.filter(n => on.includes(n)), unticked = all.filter(n => !on.includes(n));
  return "Display & Graphics → Effects: " + (ticked.length ? "tick " + ticked.join(", ") : "nothing ticked") + (unticked.length && ticked.length ? "; untick " + unticked.join(", ") : unticked.length ? " (untick all)" : "") + ".";
};

window.DR_PRESETS = [
  { id: "daily", name: "1 · Daily login", warn: "WARNING: all settings off",
    why: "Only for logging in 30+ clients at once to claim the daily. Everything off except the critical sounds: alarms, jump activations, warnings. Smallest font, no crash-report uploads.",
    values: { ...SOUND, ...LOWEST, clientFontSize: 0 },
    prefs: { breakpadUpload: false },
    manual: [effects([]),
      "Audio → Advanced Audio Settings on: Jump Activation full; Warning Sounds at half; Atmosphere, Secondary Interfaces, Ship Effects, Ship Ambience, Turret Volume and UI Click 0."] },
  { id: "alt", name: "2 · Alts / multiboxers",
    why: "The stack of alts: lowest graphics, but turrets and launchers stay on so you can read enemy guns. Same sound as everyone.",
    values: { ...SOUND, ...LOWEST },
    prefs: { bracketsAlwaysShowShipText: true },
    manual: [effects(["Turrets & Launchers", "Asteroid Environments"]) + " (Drone Models back on if this alt needs to see its drones.)", AUDIO_HAND] },
  { id: "main", name: "3 · Mains / scanners",
    why: "Commanders and the PI scanner: Shader Low (planets stand still for the scanner), Texture Medium, LOD Medium (draws guns further out), everything else low.",
    values: { ...SOUND, antiAliasing: 0, postProcessingQuality: 0, shaderQuality: 1, textureQuality: 1, lodQuality: 2, shadowQuality: 0, aoQuality: 0, volumetricQuality: 0, reflectionQuality: 0, charTextureQuality: 1, dofEnabled: 0, charClothSimulation: 0, frameGeneration: false },
    prefs: { bracketsAlwaysShowShipText: true },
    manual: [effects(["Turrets & Launchers", "Effects", "Missile Effects", "Explosions", "Drone Models", "Asteroid Environments", "Inspace Skinning Effect"]),
      "Character Display Settings: Low Quality Characters off, Greenscreen off (texture is set here: Medium).", AUDIO_HAND] },
  { id: "hq", name: "4 · High quality main", warn: "WARNING: may impact performance",
    why: "For the one main you look at most: Anti-aliasing and post-processing Medium, everything else High. Not for planet scans (the scanner needs Shader Low).",
    values: { ...SOUND, antiAliasing: 2, postProcessingQuality: 1, shaderQuality: 3, textureQuality: 0, lodQuality: 3, shadowQuality: 2, aoQuality: 2, volumetricQuality: 2, reflectionQuality: 3, charTextureQuality: 0, dofEnabled: 0, charClothSimulation: 0, frameGeneration: false },
    prefs: { bracketsAlwaysShowShipText: true },
    manual: [effects(["Turrets & Launchers", "Effects", "Missile Effects", "Explosions", "Drone Models", "Trails", "GPU Particles", "Asteroid Environments", "Inspace Skinning Effect"]), AUDIO_HAND] },
];
