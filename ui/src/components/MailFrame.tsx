import { useEffect, useRef } from "react";
import { useT } from "../lib/i18n";
import { FRAME_SANDBOX, frameColors, frameDocument, openableLink } from "../lib/mailframe";
import { openExternal } from "../lib/api";
import type { Theme } from "../lib/theme";

type Props = { html: string; allowRemote: boolean; theme: Theme };

/** The message body in a sandboxed frame (lib/mailframe.ts); links open in the browser. */
export function MailFrame({ html, allowRemote, theme }: Props) {
  const t = useT();
  const ref = useRef<HTMLIFrameElement>(null);

  useEffect(() => {
    const frame = ref.current;
    if (!frame) return;
    const fit = () => {
      const doc = frame.contentDocument;
      if (doc?.documentElement) frame.style.height = `${doc.documentElement.scrollHeight}px`;
    };
    const onLoad = () => {
      fit();
      frame.contentDocument?.addEventListener("click", (e) => {
        const a = (e.target as Element | null)?.closest?.("a");
        if (!a) return;
        e.preventDefault();
        const href = openableLink(a.getAttribute("href"));
        if (href) void openExternal(href);
      });
      for (const img of Array.from(frame.contentDocument?.images ?? [])) img.addEventListener("load", fit);
    };
    frame.addEventListener("load", onLoad);
    frame.srcdoc = frameDocument(html, allowRemote, frameColors());
    return () => frame.removeEventListener("load", onLoad);
    // The theme changes the frame's colors.
  }, [html, allowRemote, theme]);

  return <iframe ref={ref} className="mail-frame" title={t("read.frameTitle")} sandbox={FRAME_SANDBOX} referrerPolicy="no-referrer" />;
}
