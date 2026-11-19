//! EVE's "Authorized third-party applications" page for one character, in a window of the app
//! (Tsa, 19 Nov). The page shows every character of whichever EVE account is signed in there, so
//! a strip at the bottom says which account it should be and offers to switch (log out, then
//! back to the same page, where the pilot signs in with the right one). The app never signs in
//! or clicks anything there: it only reads the page's top line to compare the account name.
//! The window keeps its sign-in between uses, so EVE asks for e-mail codes less often.

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const HOST: &str = "developers.eveonline.com";
static OPENED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The page for this character; `login` is its EVE account's login name, when the app knows it.
pub fn target(character: u64) -> String {
    format!("https://{HOST}/authorized-apps?character={character}")
}

/// The strip's script (runs only on EVE's developer site, never on the login page).
pub fn strip_script(target: &str, login: Option<&str>) -> String {
    let want = serde_json::to_string(&login.unwrap_or("")).unwrap_or_else(|_| "\"\"".into());
    let target = serde_json::to_string(target).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(() => {{
  if (location.host !== '{HOST}') return;
  const want = {want}, target = {target}, back = 'dr-eveapps-back';
  try {{
    const t = sessionStorage.getItem(back);
    if (t && !location.pathname.startsWith('/logout') && !location.pathname.startsWith('/authorized-apps')) {{ sessionStorage.removeItem(back); location.replace(t); return; }}
    if (location.pathname.startsWith('/authorized-apps')) sessionStorage.removeItem(back);
  }} catch (e) {{}}
  const esc = (t) => String(t).replace(/[&<>"']/g, (c) => '&#' + c.charCodeAt(0) + ';');
  const signedInAs = () => {{
    if (!want) return null;
    const top = [...document.querySelectorAll('header, nav, [class*="user" i], [class*="account" i]')].map((e) => e.innerText || '').join(' ');
    return new RegExp('(^|[^A-Za-z0-9_.-])' + want.replace(/[.*+?^${{}}()|[\]\\]/g, '\\$&') + '($|[^A-Za-z0-9_.-])', 'i').test(top);
  }};
  const draw = () => {{
    if (!document.body) return;
    let bar = document.getElementById('dr-eveapps');
    if (!bar) {{
      bar = document.createElement('div');
      bar.id = 'dr-eveapps';
      bar.style.cssText = 'position:fixed;left:0;right:0;bottom:0;z-index:2147483647;background:#16161b;border-top:2px solid #c0392b;color:#e8e8ee;font:13px/1.4 Segoe UI,sans-serif;padding:8px 14px;display:flex;gap:10px;align-items:center;flex-wrap:wrap';
      document.body.append(bar);
      bar.addEventListener('click', (e) => {{
        if (!e.target.closest('[data-switch]')) return;
        try {{ sessionStorage.setItem(back, target); }} catch (x) {{}}
        location.href = 'https://{HOST}/logout';
      }});
    }}
    const ok = signedInAs();
    const state = ok === true ? 'ok' : ok === false ? 'wrong' : 'unknown';
    if (bar.dataset.state === state) return;
    bar.dataset.state = state;
    const sw = '<button data-switch style="background:#2a2a33;color:#fff;border:1px solid #555;border-radius:4px;padding:2px 10px;cursor:pointer">Switch account</button>';
    const tip = 'Revoke (→ then Revoke access) every app you don\'t use any more: old corps, old tools, temporary allies. Keep <b>devilsrage SeAT</b> and <b>devilsrage mapper</b>.';
    bar.innerHTML = state === 'ok'
      ? `<b style="color:#3fbf5f">✓ Signed in as ${{esc(want)}}</b><span>${{tip}}</span>`
      : state === 'wrong'
        ? `<b style="color:#f0a030">Top right should say ${{esc(want)}}.</b><span>This page shows only the signed-in account's characters.</span>${{sw}}`
        : `<span>Top right shows which EVE account is signed in; the tabs are its characters. ${{tip}}</span>${{sw}}`;
  }};
  setInterval(draw, 1200);
  addEventListener('DOMContentLoaded', draw);
}})();"#
    )
}

/// Open (or replace) the window on this character's page.
pub fn open(app: &AppHandle, character: u64, login: Option<&str>) {
    let url = target(character);
    let Ok(parsed) = url.parse() else { return };
    for (label, window) in app.webview_windows() {
        if label.starts_with("eve-apps") {
            let _ = window.close();
        }
    }
    let n = OPENED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let built = WebviewWindowBuilder::new(app, format!("eve-apps-{n}"), WebviewUrl::External(parsed))
        .title(match login {
            Some(l) if !l.is_empty() => format!("EVE third-party apps · {l}"),
            _ => "EVE third-party apps".into(),
        })
        .inner_size(1100.0, 880.0)
        .min_inner_size(600.0, 400.0)
        .center()
        .initialization_script(strip_script(&url, login))
        .build();
    match built {
        Ok(window) => {
            let _ = window.set_focus();
            log::info!("opened EVE's third-party apps page for character {character}");
        }
        Err(e) => log::warn!("couldn't open EVE's third-party apps page: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_strip_knows_its_page_and_account_and_stays_on_evess_developer_site() {
        let script = strip_script(&target(2122190460), Some("tsawind27"));
        assert!(script.contains("\"https://developers.eveonline.com/authorized-apps?character=2122190460\""));
        assert!(script.contains("\"tsawind27\""));
        assert!(script.contains("location.host !== 'developers.eveonline.com'"));
        let unknown = strip_script(&target(1), None);
        assert!(unknown.contains("const want = \"\""));
    }
}
