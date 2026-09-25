// Fixed page tap for the CDP browser host (see `cdp.rs`).
//
// This file is loaded with `include_str!` — it is NEVER generated, formatted,
// or interpolated. The bytes below are identical for every task, which is what
// makes them safe to evaluate: there is no string building, so there is no
// injection vector for source-controlled values to enter evaluated code.
//
// Purpose: record values passing through `JSON.parse` into a bounded buffer.
// Some sites serve ciphertext over the network (`{"e": blob}`) that only
// page code can decrypt; the `Network` domain sees ciphertext forever,
// so the decrypted values are observable only here.
// Matching still happens host-side in Rust (`browser_capture_matches`).
(() => {
if (window.__wefTapInstalled) return 0;
window.__wefTapInstalled = true;
window.__wefTap = [];
window.__wefTapBin = [];
function note(v) {
try {
if (v !== null && typeof v === "object" && window.__wefTap.length < 100) window.__wefTap.push(v);
} catch (e) {}
}
try {
const origParse = JSON.parse;
JSON.parse = new Proxy(origParse, {
apply(t, th, a) {
const v = Reflect.apply(t, th, a);
note(v);
return v;
}
});
} catch (e) {}
try {
const origAtob = window.atob.bind(window);
window.atob = function (value) {
const decoded = origAtob(value);
try {
if (decoded.length <= 8192 && window.__wefTapBin.length < 20) {
const bytes = [];
for (let i = 0; i < decoded.length; i++) bytes.push(decoded.charCodeAt(i) & 255);
window.__wefTapBin.push(bytes);
}
} catch (e) {}
return decoded;
};
} catch (e) {}
return 1;
})()
