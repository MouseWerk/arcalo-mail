import { useEffect, useRef } from "react";
import { useT } from "../lib/i18n";
import { FRAME_SANDBOX, frameColors, frameDocument, openableLink } from "../lib/mailframe";
import { openExternal } from "../lib/api";
import type { Theme } from "../lib/theme";

type Props = { html: string; allowRemote: boolean; theme: Theme; plain: boolean };

/** Opens `mailto:` links in the compose sheet (App listens for this event). */
export const MAILTO_EVENT = "arcalo-mail:mailto";

/** The message body in a sandboxed frame (lib/mailframe.ts); links open in the browser, mail
 *  links in the compose sheet. The frame grows with its content. */
export function MailFrame({ html, allowRemote, theme, plain }: Props) {
  const t = useT();
  const ref = useRef<HTMLIFrameElement>(null);

  useEffect(() => {
    const frame = ref.current;
    if (!frame) return;
    let observer: ResizeObserver | null = null;
    const fit = () => {
      const doc = frame.contentDocument;
      // The frame's own border counts too (border-box), else a scroll bar shows.
      if (doc?.documentElement) frame.style.height = `${Math.ceil(doc.documentElement.scrollHeight) + frame.offsetHeight - frame.clientHeight}px`;
    };
    const onLoad = () => {
      fit();
      const doc = frame.contentDocument;
      if (!doc) return;
      doc.addEventListener("click", (e) => {
        const a = (e.target as Element | null)?.closest?.("a");
        if (!a) return;
        e.preventDefault();
        const href = openableLink(a.getAttribute("href"));
        if (!href) return;
        if (href.toLowerCase().startsWith("mailto:")) window.dispatchEvent(new CustomEvent(MAILTO_EVENT, { detail: href }));
        else void openExternal(href);
      });
      for (const img of Array.from(doc.images)) img.addEventListener("load", fit);
      if (typeof ResizeObserver !== "undefined" && doc.body) {
        observer = new ResizeObserver(fit);
        observer.observe(doc.body);
      }
    };
    frame.addEventListener("load", onLoad);
    frame.srcdoc = frameDocument(html, allowRemote, frameColors(plain));
    return () => {
      frame.removeEventListener("load", onLoad);
      observer?.disconnect();
    };
    // The theme changes the frame's colors.
  }, [html, allowRemote, theme, plain]);

  return <iframe ref={ref} className={`mail-frame${plain ? " mail-frame-plain" : ""}`} title={t("read.frameTitle")} sandbox={FRAME_SANDBOX} referrerPolicy="no-referrer" />;
}
