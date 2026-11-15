function token() { return sessionStorage.getItem("pistack.token") || ""; }
function setToken(t) { sessionStorage.setItem("pistack.token", t); }
function clearToken() { sessionStorage.removeItem("pistack.token"); }

