// Is an open console held on screen? A machine page's Console tab owns a live
// websocket; a Live reload must not tear it down. The answer reads the page
// that is up right now, before a soft rebuild replaces it.
function machineHeld() {
  return !!document.querySelector("#view .tabpane canvas.screen");
}
