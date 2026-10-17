// The platform's own UI leaking into the app: what a phone's WebView draws when the page does not
// say otherwise. Measured in the page (self-contained functions, as lib/geometry.js) on every
// screen of the Android emulator test:
// - form controls (select, date and time fields, color, checkbox, radio, range, file, progress)
//   in their platform look (`appearance` not `none`), where the design system has its own;
// - alert, confirm and prompt (stubbed: they must never be called);
// - the tap highlight (transparent), text selection on buttons, rows and tabs;
// - focus rings shown without :focus-visible;
// - overscroll glow or stretch on the app's own scrolling areas, visible scrollbars;
// - the search field without autofill, autocorrect and spell check; e-mail fields with the
//   e-mail keyboard and an Enter key that says what it does;
// - text cut off (with the system's larger font size).

/** Stubs the native dialogs before the app runs (an init script); calls are noted. */
export function stubDialogs() {
  window.__nativeDialogs = [];
  for (const name of ["alert", "confirm", "prompt"]) {
    window[name] = (msg) => {
      window.__nativeDialogs.push(`${name}(${String(msg ?? "").slice(0, 80)})`);
      return name === "confirm" ? false : null;
    };
  }
}

export function nativeDialogs() {
  return window.__nativeDialogs ?? ["(the stub was not installed)"];
}

/** The findings on the screen shown now. */
export function measureStockUi() {
  const out = [];
  const W = window.innerWidth;
  const H = window.innerHeight;
  const shown = (el) => {
    const r = el.getBoundingClientRect();
    if (r.width <= 1 || r.height <= 1 || r.bottom < 0 || r.top > H || r.right < 0 || r.left > W) return false;
    for (let e = el; e; e = e.parentElement) {
      const s = getComputedStyle(e);
      if (s.display === "none" || s.visibility === "hidden" || Number(s.opacity) === 0) return false;
    }
    const s = getComputedStyle(el);
    // Visually hidden controls behind a drawn one (a switch's checkbox).
    if (s.clip === "rect(0px, 0px, 0px, 0px)" || s.clipPath === "inset(50%)") return false;
    return true;
  };
  const name = (el) => {
    const cls = typeof el.className === "string" && el.className.trim() ? `.${el.className.trim().split(/\s+/).slice(0, 2).join(".")}` : "";
    const label = (el.getAttribute("aria-label") || el.getAttribute("placeholder") || el.textContent || "").trim().replace(/\s+/g, " ").slice(0, 30);
    const type = el.type && el.tagName === "INPUT" ? `[type=${el.type}]` : "";
    return `${el.tagName.toLowerCase()}${type}${cls}${label ? ` "${label}"` : ""}`;
  };
  const transparent = (c) => c === "transparent" || /rgba\(\s*\d+,\s*\d+,\s*\d+,\s*0\s*\)/.test(c);

  // Form controls in the platform's look.
  const CONTROLS = "select, progress, meter, input[type=date], input[type=time], input[type=datetime-local], input[type=month], input[type=week], input[type=color], input[type=checkbox], input[type=radio], input[type=range], input[type=file]";
  for (const el of document.querySelectorAll(CONTROLS)) {
    if (!shown(el)) continue;
    const s = getComputedStyle(el);
    const appearance = s.appearance || s.webkitAppearance;
    if (appearance && appearance !== "none") out.push(`stock control: ${name(el)} has appearance: ${appearance}`);
    else if (el.tagName === "SELECT" || /^(date|time|datetime-local|month|week|color|file)$/.test(el.type)) out.push(`native picker: ${name(el)} opens the platform's picker`);
  }

  // The tap highlight.
  const probes = [document.body, ...[...document.querySelectorAll("button, a[href], .row, [role=tab]")].filter(shown).slice(0, 40)];
  const highlighted = probes.filter((el) => !transparent(getComputedStyle(el).webkitTapHighlightColor ?? "transparent"));
  if (highlighted.length) out.push(`tap highlight: ${highlighted.slice(0, 3).map(name).join(", ")} flash ${getComputedStyle(highlighted[0]).webkitTapHighlightColor}`);

  // Text selection on the app's chrome.
  const selectable = [...document.querySelectorAll("button, [role=button], [role=tab], .row, .folder, .inbox-tab, .settings-nav-item")]
    .filter(shown)
    .filter((el) => {
      const s = getComputedStyle(el);
      return (s.userSelect || s.webkitUserSelect) !== "none";
    });
  if (selectable.length) out.push(`selectable chrome: ${selectable.length} buttons, rows or tabs allow text selection and the long-press menu (${selectable.slice(0, 3).map(name).join(", ")})`);

  // A focus ring without :focus-visible (after a tap).
  const a = document.activeElement;
  if (a && a !== document.body && a.matches(":focus") && !a.matches(":focus-visible") && !a.matches("input, textarea, select, [contenteditable=true]")) {
    const s = getComputedStyle(a);
    if ((s.outlineStyle !== "none" && parseFloat(s.outlineWidth) > 0) || /rgb/.test(s.boxShadow) && /0px 0px 0px [1-9]/.test(s.boxShadow)) out.push(`focus ring after a tap: ${name(a)} (outline ${s.outlineStyle} ${s.outlineColor})`);
  }

  // Scrolling areas: overscroll glow/stretch, visible scrollbars.
  for (const el of document.querySelectorAll("*")) {
    const s = getComputedStyle(el);
    if (!/(auto|scroll)/.test(s.overflowY) || el.scrollHeight <= el.clientHeight + 1 || !shown(el)) continue;
    if (s.overscrollBehaviorY === "auto") out.push(`overscroll: ${name(el)} stretches or glows at its ends (overscroll-behavior-y: auto)`);
    const bar = el.offsetWidth - el.clientWidth - parseFloat(s.borderLeftWidth) - parseFloat(s.borderRightWidth);
    if (bar > 0) out.push(`scrollbar: ${name(el)} shows a ${bar} px scrollbar`);
  }
  {
    const s = getComputedStyle(document.documentElement);
    if (document.documentElement.scrollHeight > H + 1 && s.overscrollBehaviorY === "auto") out.push("overscroll: the page itself scrolls and stretches");
  }

  // Search and e-mail fields.
  for (const el of document.querySelectorAll("input[type=search], .search-input")) {
    if (!shown(el)) continue;
    const want = { autocomplete: "off", autocorrect: "off", autocapitalize: "off", spellcheck: "false" };
    const wrong = Object.entries(want).filter(([k, v]) => (el.getAttribute(k) ?? "").toLowerCase() !== v).map(([k]) => k);
    if (wrong.length) out.push(`search field ${name(el)}: ${wrong.join(", ")} not off (the keyboard suggests and autofills)`);
  }
  for (const el of document.querySelectorAll("input[type=email], input[inputmode=email], input[autocomplete~=email]")) {
    if (!shown(el)) continue;
    if (el.type !== "email" && el.inputMode !== "email") out.push(`e-mail field ${name(el)}: no e-mail keyboard (type/inputmode)`);
    if (!el.getAttribute("enterkeyhint")) out.push(`e-mail field ${name(el)}: no enterkeyhint`);
    if ((el.getAttribute("autocapitalize") ?? "") !== "off" && (el.getAttribute("autocapitalize") ?? "") !== "none") out.push(`e-mail field ${name(el)}: autocapitalize not off`);
  }
  // The recipients of a message (chips with a text field): addresses, not words.
  for (const el of document.querySelectorAll(".compose .chip-input")) {
    if (!shown(el)) continue;
    if (el.type !== "email" && el.inputMode !== "email") out.push(`recipient field ${name(el)}#${el.id}: no e-mail keyboard (inputmode=email)`);
    if ((el.getAttribute("autocapitalize") ?? "") !== "off" && (el.getAttribute("autocapitalize") ?? "") !== "none") out.push(`recipient field #${el.id}: autocapitalize not off`);
  }

  // Text cut off: labels of buttons, tabs, folders and rows that do not fit and do not end in
  // an ellipsis.
  for (const el of document.querySelectorAll("button, [role=tab], .folder-name, .row-subject, .row-from, .settings-nav-item, label, h1, h2")) {
    if (!shown(el) || !el.textContent.trim()) continue;
    const s = getComputedStyle(el);
    const clipsX = el.scrollWidth > el.clientWidth + 1 && s.overflowX !== "visible" && s.textOverflow !== "ellipsis";
    const clipsY = el.scrollHeight > el.clientHeight + 2 && s.overflowY !== "visible" && !(/(auto|scroll)/.test(s.overflowY));
    if (clipsX || clipsY) out.push(`text cut off: ${name(el)} (${el.scrollWidth}×${el.scrollHeight} in ${el.clientWidth}×${el.clientHeight})`);
  }
  return [...new Set(out)];
}
