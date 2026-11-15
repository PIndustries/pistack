function setErr(msg) {
  const n = document.getElementById("err");
  lastErr = msg || "";
  n.style.display = msg ? "block" : "none";
  n.textContent = msg || "";
}
