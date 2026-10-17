// Geometry of a phone screen, measured in the page: the checks the phone tests share, on the
// desktop app in its touch layout (WebdriverIO's `execute`) and in the Android app over the
// WebView's DevTools socket (Playwright's `evaluate`). Every check is a self-contained function
// (it runs in the page, so it uses nothing from this file) that returns plain data; `problems`
// turns the data into readable failures, empty when the screen is right.
//
// What a phone screen must be:
// - as wide as the viewport, nothing wider (no sideways scrolling of the page);
// - nothing tappable under the status bar or the navigation bar (`insets` in CSS pixels);
// - spinners small and round, never a long line across the screen;
// - the compose button clear of the last row of the list when it is scrolled to the end;
// - every tab of the sorted inbox visible, or reachable by scrolling the tab strip;
// - one bar on top while writing, with the send button in it; at most one row of tools above
//   the keyboard; no wide split send button;
// - names shown whole (a bracket is not cut off).

/** The page side of `layout`: width, overflow and what can be tapped inside the insets. */
export function measureLayout(insets) {
  const W = window.innerWidth;
  const H = window.innerHeight;
  const visible = (el) => {
    const s = getComputedStyle(el);
    if (s.display === "none" || s.visibility === "hidden" || Number(s.opacity) === 0) return false;
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  };
  const describe = (el) => {
    const r = el.getBoundingClientRect();
    const cls = typeof el.className === "string" ? el.className.trim().split(/\s+/).slice(0, 3).join(".") : "";
    const label = (el.getAttribute("aria-label") || el.textContent || "").trim().replace(/\s+/g, " ").slice(0, 40);
    return { what: `${el.tagName.toLowerCase()}${cls ? `.${cls}` : ""}${label ? ` "${label}"` : ""}`, x: Math.round(r.left), y: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height) };
  };
  const INTERACTIVE = "button, a[href], input:not([type=hidden]), select, textarea, summary, [role=button], [role=tab], [role=link], [role=menuitem], [role=switch], [role=checkbox], [contenteditable=true], [tabindex]:not([tabindex='-1'])";
  // Sideways: the document and every visible element that reaches past the right edge without
  // a container that scrolls sideways around it.
  const scrollsX = (el) => {
    for (let p = el.parentElement; p; p = p.parentElement) {
      const s = getComputedStyle(p);
      if ((s.overflowX === "auto" || s.overflowX === "scroll" || s.overflowX === "hidden" || s.overflowX === "clip") && p !== document.documentElement && p !== document.body) return true;
    }
    return false;
  };
  const wide = [];
  for (const el of document.body.querySelectorAll("*")) {
    if (wide.length >= 8) break;
    if (el.closest("#splash")) continue;
    const r = el.getBoundingClientRect();
    if ((r.right > W + 1 || r.left < -1) && r.width > 0 && visible(el) && !scrollsX(el)) wide.push(describe(el));
  }
  // What a finger reaches inside the insets: the topmost element at points across the bands.
  const inInsets = [];
  const seen = new Set();
  const scrollsY = (el) => el.scrollHeight > el.clientHeight + 1 && /(auto|scroll)/.test(getComputedStyle(el).overflowY);
  // Content may scroll under a bar as long as it can be scrolled out from under it: only what
  // stays there at the end of its scrolling counts (a scrolling area itself is no target).
  const stuck = (el) => {
    for (let p = el.parentElement; p; p = p.parentElement) {
      if (scrollsY(p)) return p.scrollTop + p.clientHeight >= p.scrollHeight - 1 && p.scrollTop <= 0 ? true : p.scrollTop + p.clientHeight >= p.scrollHeight - 1 ? "end" : false;
    }
    return true;
  };
  const probe = (x, y, band) => {
    const hit = document.elementFromPoint(x, y);
    const target = hit?.closest(INTERACTIVE);
    if (!target || seen.has(target) || target.closest("#splash")) return;
    if (scrollsY(target)) return;
    const s = stuck(target);
    // At the top band only content at the start of its scrolling is stuck, at the bottom only
    // content at the end.
    if (s === false || (s === "end" && band === "top")) return;
    seen.add(target);
    inInsets.push({ band, ...describe(target) });
  };
  const bands = [
    ["top", 0, insets.top, 0, W],
    ["bottom", H - insets.bottom, H, 0, W],
    ["left", 0, H, 0, insets.left],
    ["right", 0, H, W - insets.right, W],
  ];
  for (const [band, y0, y1, x0, x1] of bands) {
    if (y1 - y0 < 1 || x1 - x0 < 1) continue;
    for (let y = y0 + 0.5; y < y1; y += Math.max(1, Math.min(6, (y1 - y0) / 3))) {
      for (let x = x0 + 0.5; x < x1; x += Math.max(1, Math.min(8, (x1 - x0) / 3))) probe(x, y, band);
    }
  }
  // Cut off at the top: content above the screen that its container cannot scroll back to (a
  // centered box taller than its scrolling parent).
  const cut = [];
  for (const el of document.body.querySelectorAll("button, input, h1, h2, label, p")) {
    if (cut.length >= 5) break;
    if (el.closest("#splash, [aria-hidden=true]") || !visible(el)) continue;
    const r = el.getBoundingClientRect();
    if (r.top >= -1) continue;
    let p = el.parentElement;
    while (p && !(p.scrollHeight > p.clientHeight + 1 && /(auto|scroll)/.test(getComputedStyle(p).overflowY))) p = p.parentElement;
    const offscreenByScroll = p && p.scrollTop > 0 && p.getBoundingClientRect().top - r.top <= p.scrollTop + 1;
    if (!offscreenByScroll && !el.closest("[inert]")) cut.push(describe(el));
  }
  // The screen's root boxes: the app, the setup, the compose sheet and the settings.
  const roots = [];
  for (const sel of [".app", ".setup", ".compose", ".settings", ".calv-head"]) {
    for (const el of document.querySelectorAll(sel)) {
      if (!visible(el)) continue;
      const box = sel === ".calv-head" ? el.parentElement : el;
      const r = box.getBoundingClientRect();
      roots.push({ sel, left: Math.round(r.left), width: Math.round(r.width) });
    }
  }
  // The setup's card: on a phone it uses the width, not a dialog in the middle.
  const cards = [...document.querySelectorAll(".setup-card")].filter(visible).map((c) => Math.round(c.getBoundingClientRect().width));
  return {
    viewport: { w: W, h: H },
    docWidth: document.documentElement.scrollWidth,
    bodyWidth: document.body.scrollWidth,
    wide,
    cut,
    inInsets,
    roots,
    cards,
  };
}

/** Problems of a `measureLayout` result. */
export function layoutProblems(m, { setupShare = 0.9 } = {}) {
  const out = [];
  const W = m.viewport.w;
  if (m.docWidth > W + 1) out.push(`the page is ${m.docWidth} px wide in a ${W} px viewport (sideways scrolling)`);
  if (m.bodyWidth > W + 1) out.push(`the body is ${m.bodyWidth} px wide in a ${W} px viewport`);
  for (const w of m.wide) out.push(`reaches past the screen: ${w.what} at x=${w.x} w=${w.w}`);
  for (const r of m.roots) if (Math.abs(r.width - W) > 1 || Math.abs(r.left) > 1) out.push(`${r.sel} is ${r.width} px wide at x=${r.left}, the viewport ${W} px`);
  if (!m.roots.length) out.push("no screen root (.app, .setup, .compose, .settings) is visible");
  for (const c of m.cards) if (c < W * setupShare) out.push(`the setup card is ${c} px wide on a ${W} px phone (not the full width)`);
  for (const c of m.cut) out.push(`cut off above the screen, out of reach: ${c.what} at y=${c.y}`);
  for (const i of m.inInsets) out.push(`tappable inside the ${i.band} inset: ${i.what} at y=${i.y} h=${i.h}`);
  return out;
}

/** Starts recording every spinner the page shows (call once per page load). */
export function recordSpinners() {
  if (window.__spinnerLog) return;
  window.__spinnerLog = [];
  const SEL = ".spinner, [role=progressbar], .spinning svg, .splash-mark, [class*='spinner'], [class*='loader']";
  const sample = () => {
    for (const el of document.querySelectorAll(SEL)) {
      const s = getComputedStyle(el);
      if (s.display === "none" || s.visibility === "hidden") continue;
      const r = el.getBoundingClientRect();
      if (r.width === 0 && r.height === 0) continue;
      const cls = typeof el.className === "string" ? el.className : el.getAttribute("class") || "";
      const key = `${el.tagName.toLowerCase()}.${cls.trim().split(/\s+/).join(".")}|${Math.round(r.width)}x${Math.round(r.height)}`;
      if (window.__spinnerLog.length < 200 && !window.__spinnerLog.some((x) => x.key === key)) {
        window.__spinnerLog.push({ key, cls, w: Math.round(r.width * 10) / 10, h: Math.round(r.height * 10) / 10, splash: el.classList.contains("splash-mark") });
      }
    }
  };
  sample();
  window.__spinnerTimer = setInterval(sample, 40);
  new MutationObserver(sample).observe(document.documentElement, { childList: true, subtree: true, attributes: true, attributeFilter: ["class"] });
}

/** The spinners recorded so far. */
export function recordedSpinners() {
  return window.__spinnerLog ?? [];
}

/** Problems of the recorded spinners: larger than an icon, or not square. */
export function spinnerProblems(list, { max = 48, splashMax = 160 } = {}) {
  const out = [];
  for (const s of list) {
    const limit = s.splash ? splashMax : max;
    if (s.w > limit || s.h > limit) out.push(`spinner ${s.key} is ${s.w}×${s.h} px (at most ${limit})`);
    if (Math.abs(s.w - s.h) > 2) out.push(`spinner ${s.key} is not square: ${s.w}×${s.h} px`);
  }
  return out;
}

/** Scrolls the message list to its start. */
export function scrollListToStart() {
  const row = document.querySelector(".row");
  let s = row?.parentElement;
  while (s && !(s.scrollHeight > s.clientHeight + 1 && /(auto|scroll)/.test(getComputedStyle(s).overflowY))) s = s.parentElement;
  if (s) s.scrollTop = 0;
}

/** Scrolls the message list to its end (rows that load on scrolling included); returns
 *  whether the end moved, so the caller repeats until it stays. */
export function scrollListToEnd() {
  const row = document.querySelector(".row");
  if (!row) return { rows: 0, moved: false };
  let s = row.parentElement;
  while (s && !(s.scrollHeight > s.clientHeight + 1 && /(auto|scroll)/.test(getComputedStyle(s).overflowY))) s = s.parentElement;
  if (!s) return { rows: document.querySelectorAll(".row").length, moved: false };
  const before = s.scrollTop;
  s.scrollTop = s.scrollHeight;
  return { rows: document.querySelectorAll(".row").length, moved: s.scrollTop !== before };
}

/** The last row of the list and the compose button. */
export function measureFab() {
  const rows = [...document.querySelectorAll(".row:not(.row-placeholder)")].filter((r) => r.getBoundingClientRect().height > 0);
  rows.sort((a, b) => a.getBoundingClientRect().bottom - b.getBoundingClientRect().bottom);
  const last = rows.pop();
  const fab = [...document.querySelectorAll(".compose-fab")].find((f) => getComputedStyle(f).display !== "none" && f.getBoundingClientRect().height > 0);
  const box = (el) => {
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
  };
  return { last: box(last), lastText: last?.textContent.trim().slice(0, 60) ?? null, fab: box(fab), viewport: { w: window.innerWidth, h: window.innerHeight } };
}

export function fabProblems(m) {
  if (!m.last) return ["the list has no rows"];
  if (!m.fab) return [];
  const x = Math.min(m.last.right, m.fab.right) - Math.max(m.last.left, m.fab.left);
  const y = Math.min(m.last.bottom, m.fab.bottom) - Math.max(m.last.top, m.fab.top);
  if (x > 0 && y > 0) return [`the compose button covers the last row "${m.lastText}" (${Math.round(x)}×${Math.round(y)} px) with the list at its end`];
  if (m.last.bottom > m.viewport.h + 1) return [`the last row "${m.lastText}" ends below the screen (${Math.round(m.last.bottom)} > ${m.viewport.h})`];
  return [];
}

/** The tabs of the sorted inbox: where each is and whether its strip scrolls sideways. */
export function measureTabs() {
  const strip = document.querySelector(".inbox-tabs");
  const W = window.innerWidth;
  if (!strip) return { strip: null, tabs: [] };
  const s = getComputedStyle(strip);
  const scrolls = /(auto|scroll)/.test(s.overflowX) && strip.scrollWidth > strip.clientWidth;
  const sr = strip.getBoundingClientRect();
  const tabs = [...strip.querySelectorAll(".inbox-tab")].map((t) => {
    const r = t.getBoundingClientRect();
    const cs = getComputedStyle(t);
    const shown = cs.display !== "none" && cs.visibility !== "hidden" && r.width > 0 && r.height > 0;
    // Inside the strip's scroll range, and the strip inside the screen.
    const offset = r.left - sr.left + strip.scrollLeft;
    const reachable = shown && offset >= -1 && offset + r.width <= strip.scrollWidth + 1 && sr.left >= -1 && sr.right <= W + 1;
    const inView = shown && r.left >= sr.left - 1 && r.right <= sr.right + 1;
    return { tab: t.dataset.tab, label: (t.getAttribute("aria-label") || t.textContent || "").trim(), w: Math.round(r.width), h: Math.round(r.height), shown, inView, reachable };
  });
  return { strip: { w: Math.round(sr.width), scrollWidth: strip.scrollWidth, scrolls, display: s.display }, tabs };
}

export function tabProblems(m, expected) {
  const out = [];
  if (!m.strip) return ["no tab strip in the inbox"];
  if (m.strip.display === "none") out.push("the tab strip is hidden");
  for (const name of expected) {
    const t = m.tabs.find((x) => x.tab === name);
    if (!t) {
      out.push(`no tab ${name} (tabs: ${m.tabs.map((x) => x.tab).join(", ") || "none"})`);
      continue;
    }
    if (!t.shown) out.push(`the tab ${name} is not shown`);
    else if (!t.inView && !(t.reachable && m.strip.scrolls)) out.push(`the tab ${name} is neither on screen nor reachable by scrolling the tabs`);
    else if (t.w < 24 || t.h < 24) out.push(`the tab ${name} is ${t.w}×${t.h} px`);
  }
  return out;
}

/** The compose sheet: its bars, the send control and the rows of the bottom bar. */
export function measureCompose() {
  const sheet = document.querySelector(".compose");
  if (!sheet) return null;
  const visible = (el) => {
    const s = getComputedStyle(el);
    const r = el.getBoundingClientRect();
    return s.display !== "none" && s.visibility !== "hidden" && r.width > 0 && r.height > 0;
  };
  const box = (el) => {
    const r = el.getBoundingClientRect();
    return { left: Math.round(r.left), top: Math.round(r.top), w: Math.round(r.width), h: Math.round(r.height), bottom: Math.round(r.bottom) };
  };
  const sr = sheet.getBoundingClientRect();
  const head = sheet.querySelector(".compose-head");
  const foot = sheet.querySelector(".compose-foot");
  // Bars: the header and footer elements that span the sheet (a formatting toolbar is none).
  const bars = [...sheet.querySelectorAll("header, footer")].filter(visible).filter((b) => b.getBoundingClientRect().width >= sr.width * 0.8).map((b) => ({ cls: b.className, ...box(b) }));
  const sendRe = /^(senden|send)$/i;
  const sends = [...sheet.querySelectorAll("button")].filter(visible).filter((b) => sendRe.test((b.getAttribute("aria-label") || b.textContent || "").trim()));
  const send = sends[0];
  const group = send ? (send.closest(".split-btn, [role=group]") ?? send) : null;
  // Rows of the bottom bar: the distinct lines its visible items sit on.
  let footRows = 0;
  if (foot && visible(foot)) {
    const tops = [];
    for (const c of foot.querySelectorAll("button, select, input, .compose-account")) {
      if (!visible(c)) continue;
      const r = c.getBoundingClientRect();
      if (!tops.some((t) => Math.abs(t - (r.top + r.height / 2)) < 12)) tops.push(r.top + r.height / 2);
    }
    footRows = tops.length;
  }
  return {
    sheet: box(sheet),
    viewport: { w: window.innerWidth, h: window.innerHeight },
    keyboard: document.documentElement.hasAttribute("data-keyboard"),
    head: head && visible(head) ? box(head) : null,
    foot: foot && visible(foot) ? box(foot) : null,
    footRows,
    bars,
    sends: sends.length,
    send: send ? box(send) : null,
    sendGroup: group ? { split: group !== send, ...box(group) } : null,
  };
}

export function composeProblems(m, { keyboard = false } = {}) {
  if (!m) return ["no compose sheet"];
  const out = [];
  if (!m.head) out.push("no bar on top of the compose sheet");
  const topBars = m.bars.filter((b) => b.top < m.sheet.top + m.sheet.h / 3);
  if (topBars.length > 1) out.push(`${topBars.length} bars on top of the compose sheet: ${topBars.map((b) => b.cls).join(", ")}`);
  if (!m.send) out.push("no send button");
  else if (m.head && !(m.send.top >= m.head.top - 1 && m.send.bottom <= m.head.bottom + 1)) out.push(`the send button is not in the top bar (at y=${m.send.top}, the bar ${m.head.top}–${m.head.bottom})`);
  if (m.sendGroup?.split) out.push(`the send button is a split button (${m.sendGroup.w}×${m.sendGroup.h} px)`);
  if (m.sendGroup && m.sendGroup.w > 112) out.push(`the send control is ${m.sendGroup.w} px wide`);
  if (keyboard && m.footRows > 1) out.push(`the bar above the keyboard has ${m.footRows} rows`);
  if (keyboard && m.foot && m.foot.h > 64) out.push(`the bar above the keyboard is ${m.foot.h} px high`);
  return out;
}

/** The names of elements (`sel`) as shown: their text, and whether it is cut off. */
export function measureNames(sel) {
  return [...document.querySelectorAll(sel)]
    .filter((e) => e.getBoundingClientRect().width > 0)
    .map((e) => ({ text: e.textContent, clipped: e.scrollWidth > e.clientWidth + 1 }));
}

export function nameProblems(names, expected) {
  const out = [];
  for (const want of expected) {
    const hits = names.filter((n) => n.text === want);
    if (!hits.length) {
      const near = names.filter((n) => n.text.replace(/[[\]]/g, "") === want.replace(/[[\]]/g, "")).map((n) => JSON.stringify(n.text));
      out.push(`no name ${JSON.stringify(want)}${near.length ? ` (shown as ${near.join(", ")})` : ""}`);
    } else if (hits.some((h) => h.clipped)) out.push(`${JSON.stringify(want)} is cut off`);
  }
  for (const n of names) {
    const open = (n.text.match(/\[/g) || []).length;
    const close = (n.text.match(/\]/g) || []).length;
    if (open !== close) out.push(`unbalanced brackets in ${JSON.stringify(n.text)}`);
  }
  return out;
}
