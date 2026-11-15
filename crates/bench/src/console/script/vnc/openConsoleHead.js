function openConsole(card, id) {
  if (card.querySelector("canvas.screen")) return;
  const status = el("p", "Connecting", "sub");
  const canvas = el("canvas", null, "screen");
  canvas.tabIndex = 0;
  card.append(status, canvas);
  const ctx = canvas.getContext("2d");
  let ws = null;
  let buf = new Uint8Array(0);
  let off = 0;
  let stage = "version";
  let pf = null;
  let fbW = 0;
  let fbH = 0;
  let rectsLeft = 0;
  let rect = null;
  let rawLeft = 0;
  let pending = 0;
  let buttons = 0;
