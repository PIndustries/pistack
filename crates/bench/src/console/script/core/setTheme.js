function setTheme(name) {
  if (name === "light") document.documentElement.dataset.theme = "light";
  else delete document.documentElement.dataset.theme;
}
try { setTheme(localStorage.getItem("pistack.theme")); } catch (e) { /* private window */ }

// The build this server runs, under the navigation and on the sign-in
// card. The root of the API gives it to anyone.
