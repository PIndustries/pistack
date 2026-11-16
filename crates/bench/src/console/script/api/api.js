async function api(method, path, body, auth) {
  // A browser cannot be sent to another server the way pi can, so a
  // server that does not lead is asked to carry the request there.
  const headers = { "X-PiStack-Forward": "leader" };
  if (auth !== false && token()) headers.Authorization = "Bearer " + token();
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const res = await fetch(path, { method, headers, body: body !== undefined ? JSON.stringify(body) : undefined });
  const text = await res.text();
  let data = null;
  if (text) {
    try { data = JSON.parse(text); } catch (e) { data = { error: { message: text.slice(0, 200) } }; }
  }
  if (res.status === 401 && auth !== false) { showLogin(); throw new Error("session ended"); }
  if (!res.ok) {
    const err = new Error((data && data.error && data.error.message) || res.statusText);
    err.status = res.status;
    throw err;
  }
  // A change went through: the create form has done its job.
  if (method !== "GET") formOpen[current] = false;
  return data;
}

