// The document of the reading pane's frame. The HTML arrives sanitized from the backend
// (mail_core::sanitize: no scripts, forms, frames or style sheets; remote images removed unless
// allowed). The frame adds a second wall: it is sandboxed without `allow-scripts`, and the
// document carries its own Content Security Policy, which allows remote images only when the
// user chose to load them. `allow-same-origin` (without scripts nothing in the frame can use
// it) lets the app catch link clicks and open them in the browser.

export type FrameColors = { paper: string; text: string; link: string; quote: string; font: string };

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
html{background:${c.paper};color:${c.text};font-family:${c.font};font-size:14px;line-height:1.5}
body{margin:0;padding:20px 24px;overflow-wrap:anywhere}
a{color:${c.link}}
img{max-width:100%;height:auto}
blockquote{margin:0 0 0 4px;padding-left:12px;border-left:2px solid ${c.quote};color:${c.quote}}
pre,.plain{white-space:pre-wrap}
table{max-width:100%}
</style></head><body>${html}</body></html>`;
}

/** The frame's colors from the current theme's tokens. */
export function frameColors(root: HTMLElement = document.documentElement): FrameColors {
  const css = getComputedStyle(root);
  const v = (name: string) => css.getPropertyValue(name).trim();
  return { paper: v("--paper"), text: v("--paper-text"), link: v("--paper-link"), quote: v("--paper-quote"), font: v("--font-ui") };
}

/** Only web and mail links leave the frame (to the browser or the compose sheet). */
export function openableLink(href: string | null): string | null {
  if (!href) return null;
  return /^(https?:|mailto:)/i.test(href.trim()) ? href.trim() : null;
}
