  function keysym(ev) {
    const named = {
      Enter: 0xff0d, Backspace: 0xff08, Tab: 0xff09, Escape: 0xff1b,
      ArrowLeft: 0xff51, ArrowUp: 0xff52, ArrowRight: 0xff53, ArrowDown: 0xff54,
      Home: 0xff50, End: 0xff57, PageUp: 0xff55, PageDown: 0xff56,
      Insert: 0xff63, Delete: 0xffff, " ": 0x20,
      Shift: 0xffe1, Control: 0xffe3, Alt: 0xffe9, Meta: 0xffe7,
    };
    if (named[ev.key] != null) return named[ev.key];
    if (ev.key.length === 1) return ev.key.charCodeAt(0);
    return null;
  }
  function sendKey(k, down) {
    const m = new Uint8Array(8);
    m[0] = 4;
    m[1] = down;
    m[4] = (k >>> 24) & 255;
    m[5] = (k >>> 16) & 255;
    m[6] = (k >>> 8) & 255;
    m[7] = k & 255;
    send(m);
  }
  canvas.addEventListener("keydown", (ev) => {
    const k = keysym(ev);
    if (k == null) return;
    ev.preventDefault();
    sendKey(k, 1);
  });
  canvas.addEventListener("keyup", (ev) => {
    const k = keysym(ev);
    if (k == null) return;
    ev.preventDefault();
    sendKey(k, 0);
  });

