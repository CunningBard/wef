// Fixed drain for the tap in `tap_install.js` (see `cdp.rs`).
//
// Loaded with `include_str!`, never generated or interpolated: identical bytes
// for every task. Moves buffered values (parsed JSON plus binary-decode byte
// arrays) out and returns them; the host filters them with
// `browser_capture_matches` in Rust, the rest surface under the opt-in
// unmatched envelope.
(() => {
const t = window.__wefTap || [];
const b = window.__wefTapBin || [];
window.__wefTap = [];
window.__wefTapBin = [];
return t.concat(b);
})()
