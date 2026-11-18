// Patch 74: Client settings, Client tuning and Setups are one place, "Client setups", with a tab each.
(() => {
  const TABS = [
    ["profiles.html", "Client settings", "Copy one client's settings to the others, profiles, backups"],
    ["tuning.html", "Client tuning", "Sound, graphics and multibox presets, plus a setup guide"],
    ["setups.html", "Setups", "Where every client opens, each account's keys, overview and effects, compare setups"],
  ];
  const here = location.pathname.split("/").pop() || "";
  const h1 = document.querySelector("header h1");
  if (!h1) return;
  const style = document.createElement("style");
  style.textContent = `
    .setuptabs { display: flex; align-items: center; gap: 4px; }
    .setuptabs b { font-size: 18px; font-weight: 600; margin-right: 10px; }
    .setuptabs a { color: #8a8a92; text-decoration: none; padding: 4px 10px; border-radius: 6px; font-size: 14px; border: 1px solid transparent; }
    .setuptabs a:hover { color: #e6e6ea; border-color: #2c2c33; }
    .setuptabs a.on { color: #fff; border-color: #d6242c; background: #1c1013; }`;
  document.head.append(style);
  const nav = document.createElement("nav");
  nav.className = "setuptabs";
  const title = document.createElement("b");
  title.textContent = "Client setups";
  nav.append(title);
  for (const [page, name, what] of TABS) {
    const a = document.createElement("a");
    a.href = page; a.textContent = name; a.title = what;
    if (page === here) a.className = "on";
    nav.append(a);
  }
  h1.replaceWith(nav);
})();
