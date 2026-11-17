// Client tuning tiers (17 Nov 2026, Tsa): four presets shared by Client tuning and New launch group,
// plus every EVE setting in the order EVE's own Settings window shows it (DR_LAYOUT).
//
// The app writes what EVE keeps in each profile's core_public__.yaml (graphics, the volume
// sliders, music, quieter weapons, depth of field…) and prefs.ini. Everything else (Effects ticks,
// Advanced audio, camera, interface, gameplay) EVE keeps per account in its own binary files, which
// the app doesn't touch: those rows show the value to set by hand, once per account.
//
// Graphics numbers as EVE stores them: texture & character texture 0 High · 1 Medium · 2 Low;
// shader & LOD 3 High · 2 Medium · 1 Low; anti-aliasing 3 High · 2 Medium · 1 Low · 0 Disabled;
// post-processing 2 High · 1 Medium · 0 Low; shadows & ambient occlusion 2 High · 0 Disabled;
// reflections 3 High · 0 Disabled; volumetrics 2 High · 1 Medium · 0 Low.

// Same sound on every tier: music off, jump activations and warnings heard, the rest at half.
const SOUND = { masterVolume: 0.5, worldVolume: 0.5, uiGain: 0.5, evevoiceGain: 0.5, eveampGain: 0, useCombatMusic: 0, useOldJukeboxOverride: 0, suppressTurret: 1 };
const LOWEST = { antiAliasing: 0, postProcessingQuality: 0, shaderQuality: 1, textureQuality: 2, lodQuality: 1, shadowQuality: 0, aoQuality: 0, volumetricQuality: 0, reflectionQuality: 0, charTextureQuality: 2, dofEnabled: 0, charClothSimulation: 0, frameGeneration: false };

window.DR_PRESETS = [
  { id: "main", name: "1 · Mains / scanners",
    why: "Commanders and the PI scanner: Shader Low (planets stand still for the scanner), Texture Medium, LOD Medium (draws guns further out), everything else low.",
    values: { ...SOUND, antiAliasing: 0, postProcessingQuality: 0, shaderQuality: 1, textureQuality: 1, lodQuality: 2, shadowQuality: 0, aoQuality: 0, volumetricQuality: 0, reflectionQuality: 0, charTextureQuality: 1, dofEnabled: 0, charClothSimulation: 0, frameGeneration: false },
    prefs: { bracketsAlwaysShowShipText: true, breakpadUpload: false } },
  { id: "alt", name: "2 · Alts / multiboxers",
    why: "The stack of alts: lowest graphics, but turrets and launchers stay on so you can read enemy guns. Same sound as everyone.",
    values: { ...SOUND, ...LOWEST },
    prefs: { bracketsAlwaysShowShipText: true, breakpadUpload: false } },
  { id: "hq", name: "3 · High quality main", warn: "WARNING: may impact performance",
    why: "For the one main you look at most: Anti-aliasing and post-processing Medium, everything else High. Not for planet scans (the scanner needs Shader Low).",
    values: { ...SOUND, antiAliasing: 2, postProcessingQuality: 1, shaderQuality: 3, textureQuality: 0, lodQuality: 3, shadowQuality: 2, aoQuality: 2, volumetricQuality: 2, reflectionQuality: 3, charTextureQuality: 0, dofEnabled: 0, charClothSimulation: 0, frameGeneration: false },
    prefs: { bracketsAlwaysShowShipText: true, breakpadUpload: false } },
  { id: "daily", name: "4 · Daily login", warn: "WARNING: all settings off", danger: true,
    why: "Only for logging in 30+ clients at once to claim the daily. Everything off except the critical sounds: alarms, jump activations, warnings. Smallest font, no crash-report uploads.",
    values: { ...SOUND, ...LOWEST, clientFontSize: 0 },
    prefs: { breakpadUpload: false } },
];

// Every setting in EVE's Settings window, in EVE's order. A row is one of:
//   { k: "lever key", t }         written by the app (core_public__.yaml)
//   { p: "prefs.ini key", t }     written by the app (prefs.ini)
//   { t: "label", v }             set by hand; v is one value for all tiers or { main, alt, hq, daily }
const ON = "on", OFF = "off", HALF = "half";
const tiers = (main, alt, hq, daily) => ({ main, alt, hq, daily });
window.DR_LAYOUT = [
  { section: "Display & Graphics", groups: [
    { title: "Display", rows: [
      { t: "Display Mode", v: "as you have it" },
      { t: "Window Size", v: "as you have it" },
      { k: "UIScaleWindowed", t: "UI Scaling (windowed)" },
      { k: "UIScaleFullscreen", t: "UI Scaling (full screen)" },
      { t: "Display Adapter", v: "your graphics card" },
      { t: "V-Sync", v: OFF },
      { t: "Upscaling", v: OFF },
      { k: "frameGeneration", t: "Frame generation" },
    ] },
    { title: "Graphic Content Settings", rows: [
      { k: "antiAliasing", t: "Anti-Aliasing" },
      { k: "postProcessingQuality", t: "Post Processing" },
      { k: "shaderQuality", t: "Shader Quality" },
      { k: "textureQuality", t: "Texture Quality" },
      { k: "lodQuality", t: "LOD Quality" },
      { k: "shadowQuality", t: "Shadow Quality" },
      { k: "aoQuality", t: "Ambient Occlusion" },
      { k: "volumetricQuality", t: "Volumetric Effects Quality" },
      { k: "reflectionQuality", t: "Reflections" },
      { k: "brightness", t: "Brightness" },
    ] },
    { title: "Effects", rows: [
      { t: "Turrets & Launchers", v: tiers(ON, ON, ON, OFF) },
      { t: "Effects", v: tiers(ON, OFF, ON, OFF) },
      { t: "Missile Effects", v: tiers(ON, OFF, ON, OFF) },
      { t: "Explosions", v: tiers(ON, OFF, ON, OFF) },
      { t: "Drone Models", v: tiers(ON, OFF, ON, OFF) },
      { t: "Trails", v: tiers(OFF, OFF, ON, OFF) },
      { t: "GPU Particles", v: tiers(OFF, OFF, ON, OFF) },
      { t: "Asteroid Environments", v: tiers(ON, ON, ON, OFF) },
      { t: "Inspace Skinning Effect", v: tiers(ON, OFF, ON, OFF) },
      { k: "dofEnabled", t: "Depth Of Field" },
    ] },
    { title: "Character Display Settings", rows: [
      { t: "Low Quality Characters", v: tiers(OFF, OFF, OFF, ON) },
      { t: "Enable Greenscreen Portrait Background", v: OFF },
      { k: "charTextureQuality", t: "Texture Quality" },
      { k: "charClothSimulation", t: "Cloth simulation (character screen)" },
    ] },
  ] },
  { section: "Camera", groups: [
    { title: "Camera Settings", rows: [
      { t: "Sensitivity", v: "middle" },
      { t: "Inertia", v: "middle" },
      { t: "Camera Center", v: "middle" },
      { t: "Camera Shake", v: OFF },
      { t: "Camera Bobbing", v: OFF },
      { t: "Invert Zoom Direction", v: OFF },
      { t: "Offset Interface With Camera", v: OFF },
      { t: "Dynamic Camera Movement", v: ON },
      { t: "Invert Y-axis", v: OFF },
    ] },
  ] },
  { section: "Audio", groups: [
    { title: "Audio Engine Settings", rows: [
      { t: "Audio Enabled", v: ON },
      { k: "suppressTurret", t: "Quieter Weapon Sounds" },
      { k: "useOldJukeboxOverride", t: "Override dynamic music with Classic EVE music" },
      { k: "useCombatMusic", t: "Use combat music outside of combat sites" },
    ] },
    { title: "Volume Level", rows: [
      { k: "masterVolume", t: "Master Level" },
      { k: "eveampGain", t: "Music Level" },
      { k: "uiGain", t: "UI Sound Level (alarms, notifications)" },
      { k: "evevoiceGain", t: "UI Speech Level (Aura warnings)" },
      { k: "worldVolume", t: "World Level (ships, gates, wormhole activations)" },
    ] },
    { title: "Advanced", rows: [
      { t: "Advanced Audio Settings", v: ON },
      { t: "Atmosphere", v: "0" },
      { t: "Jump Activation", v: "full" },
      { t: "Secondary Interfaces", v: tiers(HALF, HALF, HALF, "0") },
      { t: "Ship Effects", v: tiers("25%", "25%", "25%", "0") },
      { t: "Ship Ambience", v: "0" },
      { t: "Turret Volume", v: "0" },
      { t: "UI Click", v: tiers(HALF, HALF, HALF, "0") },
      { t: "Warning Sounds", v: HALF },
      { t: "Inactive Client Muting", v: OFF },
    ] },
  ] },
  { section: "User Interface", groups: [
    { title: "Text", rows: [
      { k: "clientFontSize", t: "Font Size" },
      { t: "Text shadow", v: ON },
    ] },
    { title: "Tooltips", rows: [{ t: "Tooltips Delay", v: "short (just past None)" }] },
    { title: "Mouse Cursor", rows: [{ t: "Cursor Size", v: "Normal" }] },
    { title: "Windows", rows: [
      { t: "Only Stack Windows if Shift Pressed", v: OFF },
      { t: "Try to Use Existing Info Window if Any", v: ON },
    ] },
    { title: "Notifications", rows: [
      { t: "Notifications Enabled", v: ON },
      { t: "Inventory Badging Enabled", v: OFF },
    ] },
    { title: "Window Appearance", rows: [
      { t: "Enable window blur", v: ON },
      { t: "Transparency", v: OFF },
      { t: "Light Mode Transparency", v: "about a quarter" },
      { t: "Window margin size", v: "Auto" },
      { t: "Enable compact mode by default", v: ON },
    ] },
    { title: "Color Theme", rows: [
      { t: "Only tint active window's background", v: ON },
      { t: "Match theme to active ship", v: ON },
      { t: "Color Blind Mode", v: OFF },
    ] },
  ] },
  { section: "Gameplay", groups: [
    { title: "Inflight", rows: [
      { t: "Auto Target Back", v: "0 Targets" },
      { t: "Open Radial Menu With", v: "Left Mouse Button" },
      { t: "Radial Menu Delay", v: "about a third" },
      { t: "Bracket List Delay", v: "None" },
      { t: "Use compact list for brackets gathered to edges", v: ON },
      { t: "Use compact list for brackets positioned in space", v: ON },
      { t: "Disable Emergency Warp", v: OFF },
      { p: "bracketsAlwaysShowShipText", t: "Always show ship names on brackets" },
    ] },
    { title: "Dueling", rows: [{ t: "Auto Reject Invitations", v: ON }] },
    { title: "Map", rows: [{ t: "Use classic star map", v: OFF }] },
    { title: "Chat", rows: [
      { t: "On Double-Click", v: "Show Info" },
      { t: "Log Chat to File", v: ON },
      { t: "Auto Reject Invitations", v: OFF },
      { t: "Light Background", v: ON },
      { t: "Show Member List", v: ON },
      { t: "Compact Member Entries", v: ON },
      { t: "Font Size", v: "13" },
      { t: "Message Portraits", v: "No Portrait" },
    ] },
    { title: "Crashes", rows: [{ p: "breakpadUpload", t: "Upload crashes automatically" }] },
    { title: "Boarding Cinematic", rows: [{ t: "Show boarding cinematic on first-time boarding", v: OFF }] },
  ] },
  { section: "Shortcuts", groups: [
    { title: "Window, Combat, General, Navigation, Modules, Drones, Fighters, Character Creation", rows: [{ t: "Key bindings", v: "as each client has them" }] },
  ] },
  { section: "Feature Previews", groups: [
    { title: "", rows: [{ t: "New Ship HUD", v: "not enabled" }] },
  ] },
];

// The by-hand value of a row for a tier.
window.DR_HAND = (row, tier) => (row.v && typeof row.v === "object" ? row.v[tier] ?? "as you have it" : row.v);

// "Remember my settings": saved by Client tuning, offered on both pages.
try {
  const mine = JSON.parse(localStorage.getItem("dr-tuning-mine") || "null");
  if (mine && mine.values) {
    window.DR_PRESETS.push({ id: "mine", name: "★ My settings", mine: true, base: mine.base,
      why: "Remembered " + new Date(mine.at).toLocaleString() + (mine.base ? " (started from " + (window.DR_PRESETS.find(p => p.id === mine.base)?.name || mine.base) + ")" : "") + ".",
      values: mine.values, prefs: mine.prefs || {} });
  }
} catch (_) { /* storage unavailable: no saved settings */ }

// Short by-hand list for New launch group (the tier's ticks and sliders that differ by tier).
window.DR_PRESETS.forEach(p => {
  const tier = p.base || p.id;
  const pick = (title) => window.DR_LAYOUT.flatMap(s => s.groups).find(g => g.title === title).rows.filter(r => r.v !== undefined);
  const list = (title) => pick(title).map(r => r.t + " " + window.DR_HAND(r, tier)).join(", ");
  p.manual = ["Effects: " + list("Effects") + ".", "Advanced audio: " + list("Advanced") + "."];
});
