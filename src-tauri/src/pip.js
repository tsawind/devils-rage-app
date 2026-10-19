// Devil's Rage app: stands in for Chrome's Document Picture-in-Picture, which the
// app's browser (WebView2) doesn't have. The mapper's scanner pop-out (Patch 39)
// asks documentPictureInPicture.requestWindow() for a window and draws itself into
// it; here that window is an app window: pinned on top, no Windows title bar.
//
// Things a frameless window needs from the app (drag, close, resize) are asked for
// by opening a fake address, https://dr-app.invalid/<what>. The app catches it and
// opens nothing. That way the mapper page needs no access to the app at all.
(() => {
    if (window.__DR_PIP__) return;
    window.__DR_PIP__ = true;

    const SIGNAL = 'https://dr-app.invalid/';
    const INTERACTIVE = 'button, a, input, select, textarea, label, [role="button"], [role="menu"], [role="dialog"], [contenteditable]';

    const api = {
        window: null,
        async requestWindow(options) {
            if (api.window && !api.window.closed) return api.window;
            const width = Math.round((options && options.width) || 640);
            const height = Math.round((options && options.height) || 400);
            const win = window.open('about:blank', '', `popup=yes,width=${width},height=${height}`);
            if (!win) throw new DOMException('The app could not open the pop-out.', 'NotAllowedError');

            const doc = win.document;
            if (!doc.body) {
                doc.open();
                doc.write('<!doctype html><html><head><meta charset="utf-8"><title>scanner</title></head><body></body></html>');
                doc.close();
            }
            const style = doc.createElement('style');
            style.textContent = 'html, body { margin: 0; height: 100%; overflow: hidden; }';
            doc.head.appendChild(style);

            const signal = (what) => {
                try {
                    win.open(SIGNAL + what, '_blank');
                } catch (e) {
                    // window gone
                }
            };

            // Drag the window by the pop-out's top bar (anywhere that isn't a button).
            doc.addEventListener(
                'mousedown',
                (event) => {
                    if (event.button !== 0) return;
                    const target = event.target;
                    if (!target || typeof target.closest !== 'function') return;
                    if (target.closest(INTERACTIVE)) return;
                    const bar = doc.querySelector('body > .h-screen > :first-child');
                    if (!bar || !bar.contains(target)) return;
                    event.preventDefault();
                    signal('drag');
                },
                true,
            );

            // The mapper folds the map away by resizing its window.
            win.resizeBy = (dx, dy) => signal(`resize-by?dx=${Math.round(dx || 0)}&dy=${Math.round(dy || 0)}`);

            // ✕ in the pop-out: close the app window.
            const realClose = win.close.bind(win);
            win.close = () => {
                signal('close');
                setTimeout(() => {
                    try {
                        realClose();
                    } catch (e) {
                        // already closed
                    }
                }, 300);
            };

            // The mapper listens for 'pagehide' to know the pop-out is gone. Make sure it
            // hears it exactly once, however the window was closed.
            const onHide = [];
            let hidden = false;
            const fireHide = () => {
                if (hidden) return;
                hidden = true;
                api.window = null;
                for (const fn of onHide) {
                    try {
                        fn.call(win, new Event('pagehide'));
                    } catch (e) {
                        // ignore
                    }
                }
            };
            const realAdd = win.addEventListener.bind(win);
            win.addEventListener = (type, fn, opts) => {
                if (type === 'pagehide' && typeof fn === 'function') {
                    onHide.push(fn);
                    return realAdd(type, fireHide, opts);
                }
                return realAdd(type, fn, opts);
            };
            const poll = setInterval(() => {
                if (win.closed) {
                    clearInterval(poll);
                    fireHide();
                }
            }, 500);

            api.window = win;
            return win;
        },
    };

    try {
        Object.defineProperty(window, 'documentPictureInPicture', { value: api, configurable: true });
    } catch (e) {
        // leave the browser's own
    }
})();
