// A person's picture: the contact's own, the sender's BIMI logo, or Gravatar when switched on
// (the backend decides and keeps them, `picture`); initials until one is there or when there
// is none. Never anything from inside a mail.

import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { initials } from "../lib/format";
import type { Address } from "../lib/types";

const cache = new Map<string, Promise<string | null>>();

/** The picture of an address, asked once per session. */
export function pictureOf(email: string): Promise<string | null> {
  const key = email.trim().toLowerCase();
  let p = cache.get(key);
  if (!p) {
    p = api.picture(key).catch(() => null);
    cache.set(key, p);
  }
  return p;
}

/** Forgets the pictures asked so far (after the picture settings changed). */
export function forgetPictureCache() {
  cache.clear();
}

export function Avatar({ who, size = "md", className }: { who: Address | null; size?: "sm" | "md" | "lg"; className?: string }) {
  const [src, setSrc] = useState<string | null>(null);
  const email = who?.email ?? "";
  useEffect(() => {
    let alive = true;
    setSrc(null);
    if (email) void pictureOf(email).then((p) => alive && setSrc(p));
    return () => {
      alive = false;
    };
  }, [email]);
  const cls = `avatar${size === "sm" ? " avatar-sm" : size === "lg" ? " avatar-lg" : ""}${src ? " avatar-picture" : ""}${className ? ` ${className}` : ""}`;
  return (
    <span className={cls} aria-hidden>
      {src ? <img src={src} alt="" draggable={false} /> : initials(who)}
    </span>
  );
}
