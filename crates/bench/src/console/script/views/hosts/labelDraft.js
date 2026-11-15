const labelDraft = {
  host: "", rows: [], open: false,
  reset(id, rows) { this.host = id; this.rows = rows; this.open = false; },
  close() { this.host = ""; this.rows = []; this.open = false; },
};
// A label PiStack wrote itself: shown, never sent back.
