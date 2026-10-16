// The document of the reading pane's frame. The HTML arrives sanitized from the backend
// (mail_core::sanitize: no scripts, forms, frames or style sheets; remote images removed unless
// allowed). The frame adds a second wall: it is sandboxed without `allow-scripts`, and the
// document carries its own Content Security Policy, which allows remote images only when the
// user chose to load them. `allow-same-origin` (without scripts nothing in the frame can use
// it) lets the app catch link clicks and open them in the browser.

/** `pad`: the space around the message (none for plain text, which sits in the pane);
 *  `scheme`: the frame's color scheme, the app's for plain text (else WebKit paints the frame
 *  opaque white behind a transparent page). */
export type FrameColors = { paper: string; text: string; link: string; quote: string; rule: string; font: string; pad: string; scheme: string };

export const FRAME_SANDBOX = "allow-same-origin";

export function frameCsp(allowRemote: boolean): string {
  const img = allowRemote ? "img-src data: cid: https: http:" : "img-src data: cid:";
  return `default-src 'none'; ${img}; style-src 'unsafe-inline'; font-src data:`;
}

const escapeAttr = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");

/** The `srcdoc` of the frame. */
export function frameDocument(html: string, allowRemote: boolean, c: FrameColors): string {
  return `<!doctype html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="${escapeAttr(frameCsp(allowRemote))}">
<meta name="referrer" content="no-referrer">
<style>
html{color-scheme:${c.scheme};background:${c.paper};color:${c.text};font-family:${c.font};font-size:14px;line-height:1.5}
body{margin:0;padding:${c.pad};overflow-wrap:anywhere}
a{color:${c.link}}
img{max-width:100%;height:auto}
img:not([src]){display:none}
blockquote{margin:0 0 0 2px;padding-left:12px;border-left:2px solid ${c.rule};color:${c.quote}}
pre,.plain{white-space:pre-wrap}
table{max-width:100%}
</style></head><body>${html}</body></html>`;
}

/** The frame's colors from the current theme's tokens: HTML mail on its light paper, plain
 *  text in the app's own colors (dark in the dark themes). */
export function frameColors(plain: boolean, root: HTMLElement = document.documentElement): FrameColors {
  const css = getComputedStyle(root);
  const v = (name: string) => css.getPropertyValue(name).trim();
  if (plain) return { paper: "transparent", text: v("--text"), link: v("--accent-text"), quote: v("--text-3"), rule: v("--border-strong"), font: v("--font-ui"), pad: "2px 0", scheme: css.colorScheme || "light" };
  return { paper: v("--paper"), text: v("--paper-text"), link: v("--paper-link"), quote: v("--paper-quote"), rule: v("--paper-rule"), font: v("--font-ui"), pad: "20px 24px", scheme: "light" };
}

/** Only web and mail links leave the frame (to the browser or the compose sheet). */
export function openableLink(href: string | null): string | null {
  if (!href) return null;
  return /^(https?:|mailto:)/i.test(href.trim()) ? href.trim() : null;
}
