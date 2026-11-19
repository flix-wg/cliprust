// Adds the page origin (`https://example.com`) to every copy as
// `text/x-cliprust-source`. Firefox stores custom types in
// `org.mozilla.custom-clipdata`, where ClipRust reads it. Pages you paste into
// can read custom types too, so only the origin goes there, never the path or
// query.
"use strict";

const SOURCE_TYPE = "text/x-cliprust-source";

// The page's own listeners on the element and document run first; this one
// runs as the event bubbles out of the window.
window.addEventListener("copy", (event) => {
  const data = event.clipboardData;
  const origin = location.origin;
  if (!data || !/^https?:\/\//.test(origin)) return;
  if (!event.defaultPrevented) {
    // Firefox ignores clipboardData unless the event is cancelled, so put the
    // selection there ourselves, the way the default copy would.
    const copied = selection();
    if (!copied) return;
    data.setData("text/plain", copied.text);
    if (copied.html) data.setData("text/html", copied.html);
    event.preventDefault();
  }
  data.setData(SOURCE_TYPE, origin);
});

function selection() {
  const el = document.activeElement;
  if (el && (el.localName === "input" || el.localName === "textarea")) {
    if (el.type === "password") return null;
    let text;
    try {
      text = el.value.slice(el.selectionStart, el.selectionEnd);
    } catch {
      return null; // input types without a text selection
    }
    return text ? { text, html: null } : null;
  }
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed) return null;
  const text = sel.toString();
  if (!text) return null;
  // An inert document, so nothing in the copy loads or runs.
  const box = document.implementation.createHTMLDocument("").body;
  for (let i = 0; i < sel.rangeCount; i++) box.append(sel.getRangeAt(i).cloneContents());
  // Like Firefox's own copy: no scripts or styles, and absolute links.
  for (const el of box.querySelectorAll("script, style, noscript, template")) el.remove();
  for (const el of box.querySelectorAll("[href], [src]")) {
    for (const attr of ["href", "src"]) {
      const value = el.getAttribute(attr);
      if (value === null) continue;
      try {
        el.setAttribute(attr, new URL(value, document.baseURI).href);
      } catch {
        // keep values that are not URLs
      }
    }
  }
  return { text, html: box.innerHTML };
}
