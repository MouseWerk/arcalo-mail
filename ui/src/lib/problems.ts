// What a connection problem (mail_core::network::Problem) means for the user: a title, a hint
// what to do, and the server's own words as the detail.

import type { TKey, TVars } from "./i18n";
import type { Discovered, Problem } from "./types";

export type Explained = { title: TKey; hint: TKey; vars: TVars; detail: string | null };

export function explain(p: Problem): Explained {
  switch (p.kind) {
    case "dns":
      return { title: "problem.dns", hint: "problem.dnsHint", vars: { host: p.host }, detail: null };
    case "refused":
      return { title: "problem.refused", hint: "problem.refusedHint", vars: { host: p.host, port: p.port }, detail: null };
    case "timeout":
      return { title: "problem.timeout", hint: "problem.timeoutHint", vars: { host: p.host || "–", port: p.port }, detail: null };
    case "offline":
      return { title: "problem.offline", hint: "problem.offlineHint", vars: {}, detail: null };
    case "tls":
      return { title: "problem.tls", hint: "problem.tlsHint", vars: { host: p.host }, detail: p.detail };
    case "certificate":
      return { title: "problem.certificate", hint: "problem.certificateHint", vars: { host: p.host }, detail: p.detail };
    case "auth":
      return { title: "problem.auth", hint: "problem.authHint", vars: {}, detail: p.detail };
    case "app_password":
      return { title: "problem.appPassword", hint: "problem.appPasswordHint", vars: {}, detail: p.detail };
    case "protocol":
      return { title: "problem.protocol", hint: "problem.protocolHint", vars: {}, detail: p.detail };
    case "admin_consent":
      return { title: "problem.adminConsent", hint: "problem.adminConsentHint", vars: {}, detail: p.detail };
    case "smtp_auth_disabled":
      return { title: "problem.smtpAuthDisabled", hint: "problem.smtpAuthDisabledHint", vars: {}, detail: p.detail };
    case "proxy":
      return { title: "problem.proxy", hint: "problem.proxyHint", vars: { proxy: p.proxy }, detail: p.detail };
  }
}

/** The provider's note before the password is entered. */
export function hintKey(h: Discovered["hint"]): TKey | null {
  switch (h) {
    case "app_password":
      return "setup.hintAppPassword";
    case "enable_imap":
      return "setup.hintEnableImap";
    case "mail_password":
      return "setup.hintMailPassword";
    case "microsoft_sign_in":
    case "google_app_password":
      // Shown as the Microsoft step and the Gmail guide instead of a note.
      return null;
    default:
      return null;
  }
}
