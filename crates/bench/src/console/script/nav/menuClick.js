document.getElementById("menu").addEventListener("click", () => {
  document.getElementById("shell").classList.toggle("nav-open");
});

// Typing in the box above the navigation narrows it. Enter opens the first
// match, and "/" anywhere outside a field puts the cursor there.
