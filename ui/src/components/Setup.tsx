import { ArrowLeft, CircleCheck, CircleX, ExternalLink, Info, KeyRound, Lock, LogIn, ServerCog, ShieldCheck, Sparkles, UserPlus } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useT, type TKey } from "../lib/i18n";
import { api, openExternal } from "../lib/api";
import { explain, hintKey } from "../lib/problems";
import type { Account, AccountDraft, CheckResult, Discovered, ImapTransport, MicrosoftSignIn, Pop3Transport, Problem, Security, Server } from "../lib/types";
import { Segmented, Spinner, Switch } from "./ui";

type Step =
  | { kind: "form" }
  | { kind: "discovering" }
  | { kind: "checking"; found: Discovered | null; jmap: boolean }
  | { kind: "failed"; result: CheckResult; found: Discovered | null; jmap: boolean }
  | { kind: "microsoft"; waiting: boolean; result: MicrosoftSignIn | null; adding?: boolean }
  | { kind: "done"; account: Account };

type JmapManual = { session_url: string; username: string; token: boolean };

const SOURCE: Record<Discovered["source"], TKey> = {
  preset: "setup.sourcePreset",
  autoconfig: "setup.sourceAutoconfig",
  ispdb: "setup.sourceIspdb",
  srv: "setup.sourceSrv",
  mx: "setup.sourceMx",
  guess: "setup.sourceGuess",
};

/** Microsoft's own domains (business domains are found by their MX records). */
const MICROSOFT_DOMAINS = ["outlook.com", "outlook.de", "outlook.at", "hotmail.com", "hotmail.de", "live.com", "live.de", "msn.com"];
const GOOGLE_DOMAINS = ["gmail.com", "googlemail.com"];
export const GOOGLE_APP_PASSWORDS = "https://myaccount.google.com/apppasswords";

const domainOf = (email: string) => email.trim().toLowerCase().split("@")[1] ?? "";

const defaultPort = (kind: "imap" | "smtp" | "pop3", s: Security) =>
  kind === "imap" ? (s === "tls" ? 993 : 143) : kind === "pop3" ? (s === "tls" ? 995 : 110) : s === "tls" ? 465 : 587;

function ServerFields({ kind, server, onChange }: { kind: "imap" | "smtp" | "pop3"; server: Server; onChange: (s: Server) => void }) {
  const t = useT();
  const id = `setup-${kind}`;
  return (
    <fieldset className="server-fields">
      <legend>{kind === "imap" ? t("setup.incoming") : kind === "pop3" ? t("setup.incomingPop3") : t("setup.outgoing")}</legend>
      <label className="stack-field server-host">
        <span className="field-label">{t("setup.host")}</span>
        <input id={`${id}-host`} className="input" value={server.host} spellCheck={false} onChange={(e) => onChange({ ...server, host: e.target.value.trim(), trusted_cert: null })} />
      </label>
      <label className="stack-field server-port">
        <span className="field-label">{t("setup.port")}</span>
        <input
          id={`${id}-port`}
          className="input"
          inputMode="numeric"
          value={server.port || ""}
          onChange={(e) => onChange({ ...server, port: Number(e.target.value.replace(/\D/g, "")) || 0 })}
        />
      </label>
      <label className="stack-field server-security">
        <span className="field-label">{t("setup.security")}</span>
        <select
          id={`${id}-security`}
          className="select"
          value={server.security}
          onChange={(e) => {
            const s = e.target.value as Security;
            onChange({ ...server, security: s, port: defaultPort(kind, s) });
          }}
        >
          <option value="tls">{t("setup.tls")}</option>
          <option value="start_tls">{t("setup.starttls")}</option>
          <option value="plain">{t("setup.plain")}</option>
        </select>
      </label>
    </fieldset>
  );
}

type Which = "imap" | "smtp" | "jmap" | "microsoft";

const WHICH: Record<Which, TKey> = { imap: "setup.imapLabel", smtp: "setup.smtpLabel", jmap: "setup.jmapLabel", microsoft: "setup.microsoftLabel" };

export function ProblemCard({ which, p, onTrust }: { which: Which; p: Problem; onTrust?: () => void }) {
  const t = useT();
  const e = explain(p);
  return (
    <div className="problem" role="alert">
      <CircleX size={18} strokeWidth={1.75} aria-hidden className="problem-icon" />
      <div className="problem-body">
        <p className="problem-title">
          {t(WHICH[which])}: {t(e.title, e.vars)}
        </p>
        <p className="problem-hint">{t(e.hint, e.vars)}</p>
        {p.kind === "certificate" && onTrust && (
          <div className="fingerprint">
            <span className="field-label">{t("setup.fingerprint")}</span>
            <code>{p.fingerprint}</code>
            <button type="button" className="btn btn-sm" onClick={onTrust}>
              <ShieldCheck size={14} strokeWidth={1.75} aria-hidden />
              <span>{t("setup.trust")}</span>
            </button>
          </div>
        )}
        {e.detail && <p className="problem-detail">{t("setup.serverSaid", { text: e.detail })}</p>}
      </div>
    </div>
  );
}

/** How to create a Gmail app password (Google sign-in comes later, docs/decisions.md). */
export function GmailGuide() {
  const t = useT();
  return (
    <section className="guide" aria-labelledby="gmail-guide-title">
      <div className="guide-head">
        <KeyRound size={16} strokeWidth={1.75} aria-hidden />
        <h3 id="gmail-guide-title">{t("gmail.title")}</h3>
      </div>
      <ol className="guide-steps">
        <li>{t("gmail.step2fa")}</li>
        <li>{t("gmail.stepOpen")}</li>
        <li>{t("gmail.stepName")}</li>
        <li>{t("gmail.stepPaste")}</li>
      </ol>
      <div className="guide-foot">
        <button type="button" className="btn btn-sm" onClick={() => void openExternal(GOOGLE_APP_PASSWORDS)}>
          <ExternalLink size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("gmail.open")}</span>
        </button>
        <span className="muted">{t("gmail.later")}</span>
      </div>
    </section>
  );
}

/** The first start offers the Screener once (off by default); the settings keep it. */
function ScreenerOffer() {
  const t = useT();
  const [on, setOn] = useState<boolean | null>(null);
  useEffect(() => {
    void api.settings().then((s) => setOn(s.screener));
  }, []);
  if (on === null) return null;
  const change = (v: boolean) => {
    setOn(v);
    void api.settings().then((s) => api.saveSettings({ ...s, screener: v }));
  };
  return (
    <div className="setup-offer">
      <span className="setup-offer-icon" aria-hidden>
        <UserPlus size={18} strokeWidth={1.75} />
      </span>
      <div className="setup-offer-text">
        <div className="set-label">{t("settings.screener")}</div>
        <div className="set-desc">{t("setup.screenerText")}</div>
      </div>
      <Switch label={t("settings.screener")} checked={on} onChange={change} />
    </div>
  );
}

/** Adding an account: the address and password, the servers found for it (JMAP first where
 *  the provider offers it), the connection check with clear errors, the servers by hand when
 *  nothing was found, and Microsoft's sign-in in the browser. `welcome`: the first start, with a
 *  short introduction. */
export function Setup({ welcome, onDone, onCancel }: { welcome: boolean; onDone: (a: Account) => void; onCancel?: () => void }) {
  const t = useT();
  const [name, setName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [step, setStep] = useState<Step>({ kind: "form" });
  const [transport, setTransport] = useState<ImapTransport | null>(null);
  const [found, setFound] = useState<Discovered | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [manual, setManual] = useState(false);
  const [protocol, setProtocol] = useState<"imap" | "pop3" | "jmap">("imap");
  const [pop3, setPop3] = useState<Pop3Transport | null>(null);
  const [jmapManual, setJmapManual] = useState<JmapManual>({ session_url: "", username: "", token: false });
  const [microsoftBuilt, setMicrosoftBuilt] = useState(true);
  // Each Microsoft sign-in counts up; leaving it (Back, Cancel, closing) counts up too, so a
  // browser sign-in that finishes later is dropped.
  const signInRun = useRef(0);
  useEffect(
    () => () => {
      signInRun.current++;
    },
    [],
  );

  useEffect(() => {
    void api.appInfo().then((i) => setMicrosoftBuilt(i.microsoft_sign_in ?? false));
  }, []);

  const domain = domainOf(email);
  const microsoftDomain = MICROSOFT_DOMAINS.includes(domain);
  const google = GOOGLE_DOMAINS.includes(domain) || found?.hint === "google_app_password";

  const add = async (draft: AccountDraft) => {
    const account = await api.addAccount(draft, password);
    setStep({ kind: "done", account });
  };

  const check = async (tr: ImapTransport | Pop3Transport, d: Discovered | null) => {
    if (tr.kind === "imap") setTransport(tr);
    setStep({ kind: "checking", found: d, jmap: false });
    setError(null);
    try {
      const result = await api.check(tr, password);
      if (result.imap || result.smtp || result.error) {
        setStep({ kind: "failed", result, found: d, jmap: false });
        return;
      }
      await add({ provider: "imap", name: "", email: email.trim(), sender_name: name.trim(), transport: tr });
    } catch (e) {
      setError(String(e));
      setStep({ kind: "form" });
    }
  };

  const checkJmap = async (sessionUrl: string, username: string, token: boolean, d: Discovered | null) => {
    setStep({ kind: "checking", found: d, jmap: true });
    setError(null);
    try {
      const result = await api.checkJmap(sessionUrl, username, password, token);
      if (result.imap || result.error) {
        setStep({ kind: "failed", result, found: d, jmap: true });
        return;
      }
      await add({
        provider: "jmap",
        name: "",
        email: email.trim(),
        sender_name: name.trim(),
        transport: { kind: "jmap", session_url: sessionUrl, username, auth: token ? { kind: "token" } : { kind: "password" } },
      });
    } catch (e) {
      setError(String(e));
      setStep({ kind: "form" });
    }
  };

  const imapOf = (d: Discovered): ImapTransport => ({ kind: "imap", imap: d.imap, smtp: d.smtp, username: d.username, auth: { kind: "password" } });

  const microsoft = async () => {
    setError(null);
    if (!microsoftBuilt) {
      setStep({ kind: "microsoft", waiting: false, result: null });
      return;
    }
    const mine = ++signInRun.current;
    setStep({ kind: "microsoft", waiting: true, result: null });
    try {
      const r = await api.microsoftSignIn(email.trim());
      // Left meanwhile (Back, Cancel, the dialog closed): the sign-in adds nothing.
      if (mine !== signInRun.current) return;
      if (r.session && r.check && !r.check.imap && !r.check.smtp && !r.check.error) {
        await addMicrosoft(r);
        return;
      }
      setStep({ kind: "microsoft", waiting: false, result: r });
    } catch (e) {
      if (mine !== signInRun.current) return;
      setStep({ kind: "microsoft", waiting: false, result: { session: null, email: null, check: null, problem: null, error: String(e) } });
    }
  };

  const addMicrosoft = async (r: MicrosoftSignIn) => {
    if (!r.session) return;
    const mine = signInRun.current;
    setStep({ kind: "microsoft", waiting: false, result: r, adding: true });
    try {
      const account = await api.addMicrosoftAccount(r.session, name.trim());
      if (mine === signInRun.current) setStep({ kind: "done", account });
    } catch (e) {
      setStep({ kind: "microsoft", waiting: false, result: { ...r, error: String(e) } });
    }
  };

  const start = async () => {
    if (!email.trim()) return;
    if (microsoftDomain) {
      void microsoft();
      return;
    }
    if (!password) return;
    setError(null);
    setStep({ kind: "discovering" });
    try {
      const d = await api.discover(email.trim());
      setFound(d);
      if (d.hint === "microsoft_sign_in") {
        void microsoft();
        return;
      }
      if (d.jmap) await checkJmap(d.jmap, email.trim(), false, d);
      else await check(imapOf(d), d);
    } catch (e) {
      setError(String(e));
      setStep({ kind: "form" });
    }
  };

  const trust = (which: "imap" | "smtp", p: Problem) => {
    if (p.kind !== "certificate") return;
    if (manual && protocol === "pop3" && pop3) {
      const field = which === "imap" ? "pop3" : "smtp";
      const next = { ...pop3, [field]: { ...pop3[field], trusted_cert: p.fingerprint } };
      setPop3(next);
      void check(next, found);
      return;
    }
    if (!transport) return;
    const next = { ...transport, [which]: { ...transport[which], trusted_cert: p.fingerprint } };
    void check(next, found);
  };

  const hint = found ? hintKey(found.hint) : null;
  const busy = step.kind === "discovering" || step.kind === "checking";

  if (step.kind === "done")
    return (
      <div className="setup">
        <div className="setup-card setup-done">
          <CircleCheck size={36} strokeWidth={1.5} aria-hidden className="setup-done-icon" />
          <h1>{t("setup.doneTitle")}</h1>
          <p className="muted">{t("setup.doneText", { email: step.account.email })}</p>
          {welcome && <ScreenerOffer />}
          <button type="button" className="btn btn-primary btn-lg" autoFocus onClick={() => onDone(step.account)}>
            {t("setup.toInbox")}
          </button>
        </div>
      </div>
    );

  const intro = welcome && (
    <div className="setup-intro">
      <svg className="setup-logo" viewBox="0 0 1000 1000" aria-hidden>
        {/* Arcalo Mail's mark (docs/brand/arcalo-mail-mark.svg). */}
        <path className="setup-logo-seal" d="M500 412.7 618 551.4 500 690 382 551.4Z" />
        <path className="setup-logo-ribbon" d="M100 220H900V780H100Z M100 220 500 690 900 220 M382 551.4 500 412.7 618 551.4" />
      </svg>
      <h1>{t("setup.welcome")}</h1>
      <p className="muted">{t("setup.welcomeText")}</p>
      <ul className="setup-points">
        <li>
          <Lock size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("setup.pointLocal")}</span>
        </li>
        <li>
          <ShieldCheck size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("setup.pointPrivate")}</span>
        </li>
        <li>
          <Sparkles size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("setup.pointAuto")}</span>
        </li>
      </ul>
    </div>
  );

  if (step.kind === "microsoft") {
    const r = step.result;
    const smtpOnly = !!(r?.session && r.check && !r.check.imap && !r.check.error && r.check.smtp);
    return (
      <div className={`setup${welcome ? " setup-welcome" : ""}`}>
        <div className="setup-card">
          {intro}
          <div className="setup-form setup-microsoft">
            <h2 className="setup-title">{t("ms.title")}</h2>
            {!microsoftBuilt ? (
              <>
                <div className="notice" role="note">
                  <Info size={16} strokeWidth={1.75} aria-hidden />
                  <span>{t("ms.notInBuild")}</span>
                </div>
                <p className="muted setup-text">{t("ms.appPasswordWay")}</p>
              </>
            ) : (
              <>
                <p className="setup-text">{t("ms.text")}</p>
                <ul className="setup-points">
                  <li>
                    <Lock size={14} strokeWidth={1.75} aria-hidden />
                    <span>{t("ms.pointPassword")}</span>
                  </li>
                  <li>
                    <KeyRound size={14} strokeWidth={1.75} aria-hidden />
                    <span>{t("ms.pointAccess")}</span>
                  </li>
                </ul>
                {(step.waiting || step.adding) && (
                  <div className="setup-progress" role="status">
                    <Spinner />
                    <span>{step.waiting ? t("ms.waiting") : t("setup.checking")}</span>
                  </div>
                )}
                {r?.problem && (
                  <div className="problems">
                    <ProblemCard which="microsoft" p={r.problem} />
                  </div>
                )}
                {r?.check && (
                  <div className="problems">
                    {r.check.imap && <ProblemCard which="imap" p={r.check.imap} />}
                    {r.check.smtp && <ProblemCard which="smtp" p={r.check.smtp} />}
                    {r.check.error && <p className="field-error">{r.check.error}</p>}
                  </div>
                )}
                {r?.error && (
                  <div className="notice notice-danger" role="alert">
                    <span>{r.error}</span>
                  </div>
                )}
                {smtpOnly && <p className="muted setup-text">{t("ms.addAnyway")}</p>}
              </>
            )}
            <div className="setup-actions">
              <button
                type="button"
                className="btn"
                onClick={() => {
                  signInRun.current++;
                  setStep({ kind: "form" });
                }}
              >
                <ArrowLeft size={14} strokeWidth={1.75} aria-hidden />
                <span>{microsoftBuilt ? t("common.back") : t("ms.useAppPassword")}</span>
              </button>
              <span className="toolbar-gap" />
              {smtpOnly && r && (
                <button type="button" className="btn" disabled={step.adding} onClick={() => void addMicrosoft(r)}>
                  {t("ms.addAnywayButton")}
                </button>
              )}
              {microsoftBuilt && (
                <button type="button" className="btn btn-primary" disabled={step.waiting || step.adding} onClick={() => void microsoft()}>
                  <LogIn size={14} strokeWidth={1.75} aria-hidden />
                  <span>{r ? t("setup.tryAgain") : t("ms.openBrowser")}</span>
                </button>
              )}
            </div>
          </div>
        </div>
      </div>
    );
  }

  const viaJmap = (step.kind === "checking" || step.kind === "failed") && step.jmap;
  const status = (
    <>
      {hint && (
        <div className="notice" role="note">
          <Info size={16} strokeWidth={1.75} aria-hidden />
          <span>{t(hint)}</span>
        </div>
      )}
      {error && (
        <div className="notice notice-danger" role="alert">
          <span>{error}</span>
        </div>
      )}
      {step.kind === "failed" && (
        <div className="problems">
          {step.result.imap && (
            <ProblemCard which={step.jmap ? "jmap" : "imap"} p={step.result.imap} onTrust={step.jmap ? undefined : () => step.result.imap && trust("imap", step.result.imap)} />
          )}
          {step.result.smtp && <ProblemCard which="smtp" p={step.result.smtp} onTrust={() => step.result.smtp && trust("smtp", step.result.smtp)} />}
          {step.result.error && <p className="field-error">{step.result.error}</p>}
        </div>
      )}
      {busy && (
        <div className="setup-progress" role="status">
          <Spinner />
          <span>{step.kind === "discovering" ? t("setup.discovering") : t("setup.checking")}</span>
        </div>
      )}
      {found && !manual && step.kind !== "discovering" && (
        <p className="setup-found muted">
          <ServerCog size={13} strokeWidth={1.75} aria-hidden />
          <span>
            {viaJmap ? t("setup.foundJmap") : t("setup.found", { imap: found.imap.host, smtp: found.smtp.host })} {t(SOURCE[found.source], { provider: found.provider ?? "" })}
          </span>
        </p>
      )}
      {step.kind === "failed" && step.jmap && found && !manual && (
        <button type="button" className="btn btn-quiet btn-sm setup-alt" onClick={() => void check(imapOf(found), found)}>
          {t("setup.useImapInstead")}
        </button>
      )}
    </>
  );
  return (
    <div className={`setup${welcome ? " setup-welcome" : ""}`}>
      <div className="setup-card">
        {intro}
        <div className="setup-form">
          <h2 className="setup-title">{t("setup.title")}</h2>
          {!manual && (
            <form
              className="setup-fields"
              onSubmit={(e) => {
                e.preventDefault();
                void start();
              }}
            >
              <label className="stack-field">
                <span className="field-label">{t("setup.name")}</span>
                <input className="input" value={name} placeholder={t("setup.namePlaceholder")} onChange={(e) => setName(e.target.value)} readOnly={busy} autoComplete="name" />
              </label>
              <label className="stack-field">
                <span className="field-label">{t("setup.email")}</span>
                <input
                  className="input"
                  type="email"
                  value={email}
                  autoFocus
                  placeholder={t("setup.emailPlaceholder")}
                  onChange={(e) => setEmail(e.target.value)}
                  readOnly={busy}
                  autoComplete="email"
                  spellCheck={false}
                />
              </label>
              {!microsoftDomain && (
                <>
                  {google && <GmailGuide />}
                  <label className="stack-field">
                    <span className="field-label">{google ? t("gmail.passwordLabel") : t("setup.password")}</span>
                    <input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} readOnly={busy} autoComplete="current-password" />
                  </label>
                  <p className="setup-note muted">
                    <KeyRound size={13} strokeWidth={1.75} aria-hidden />
                    <span>{t("setup.passwordNote")}</span>
                  </p>
                </>
              )}
              {microsoftDomain && (
                <p className="setup-note muted">
                  <Info size={13} strokeWidth={1.75} aria-hidden />
                  <span>{t("ms.detected")}</span>
                </p>
              )}
              {status}
              <div className="setup-actions">
                {onCancel && (
                  <button type="button" className="btn" onClick={onCancel} disabled={busy}>
                    {t("common.cancel")}
                  </button>
                )}
                <button
                  type="button"
                  className="btn btn-quiet"
                  disabled={busy}
                  onClick={() => {
                    setTransport(
                      transport ?? {
                        kind: "imap",
                        imap: found?.imap ?? { host: domain ? `imap.${domain}` : "", port: 993, security: "tls" },
                        smtp: found?.smtp ?? { host: domain ? `smtp.${domain}` : "", port: 587, security: "start_tls" },
                        username: found?.username ?? email.trim(),
                        auth: { kind: "password" },
                      },
                    );
                    setJmapManual({ session_url: found?.jmap ?? (domain ? `https://${domain}/.well-known/jmap` : ""), username: email.trim(), token: false });
                    setManual(true);
                  }}
                >
                  {t("setup.manual")}
                </button>
                <span className="toolbar-gap" />
                <button type="submit" className="btn btn-primary" disabled={busy || !email.trim() || (!microsoftDomain && !password)}>
                  {microsoftDomain ? t("ms.button") : step.kind === "failed" ? t("setup.tryAgain") : t("setup.continue")}
                </button>
              </div>
              {!microsoftDomain && (
                <div className="setup-other">
                  <span className="setup-or">{t("setup.or")}</span>
                  <button type="button" className="btn setup-ms" disabled={busy} onClick={() => void microsoft()}>
                    <LogIn size={14} strokeWidth={1.75} aria-hidden />
                    <span>{t("ms.button")}</span>
                  </button>
                </div>
              )}
            </form>
          )}
          {manual && transport && (
            <form
              className="setup-fields"
              onSubmit={(e) => {
                e.preventDefault();
                if (protocol === "jmap") void checkJmap(jmapManual.session_url.trim(), jmapManual.username.trim(), jmapManual.token, found);
                else if (protocol === "pop3") pop3 && void check(pop3, found);
                else void check(transport, found);
              }}
            >
              <Segmented<"imap" | "pop3" | "jmap">
                label={t("setup.protocol")}
                value={protocol}
                onChange={(p) => {
                  setProtocol(p);
                  if (p === "pop3" && !pop3) void api.pop3Preset(email.trim()).then((tr) => tr && setPop3(tr));
                }}
                options={[
                  ["imap", t("setup.protocolImap")],
                  ["pop3", t("setup.protocolPop3")],
                  ["jmap", t("setup.protocolJmap")],
                ]}
              />
              {protocol === "pop3" ? (
                pop3 && (
                  <>
                    <p className="muted setup-note">{t("setup.pop3Note")}</p>
                    <label className="stack-field">
                      <span className="field-label">{t("setup.username")}</span>
                      <input className="input" value={pop3.username} spellCheck={false} onChange={(e) => setPop3({ ...pop3, username: e.target.value })} />
                    </label>
                    <label className="stack-field">
                      <span className="field-label">{t("setup.password")}</span>
                      <input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} readOnly={busy} />
                    </label>
                    <ServerFields kind="pop3" server={pop3.pop3} onChange={(s) => setPop3({ ...pop3, pop3: s })} />
                    <ServerFields kind="smtp" server={pop3.smtp} onChange={(smtp) => setPop3({ ...pop3, smtp })} />
                    <div className="setup-switch">
                      <Switch label={t("setup.leaveOnServer")} checked={pop3.leave_on_server} onChange={(leave_on_server) => setPop3({ ...pop3, leave_on_server })} />
                      <span>{t("setup.leaveOnServer")}</span>
                    </div>
                    {pop3.leave_on_server && (
                      <label className="stack-field">
                        <span className="field-label">{t("setup.deleteAfter")}</span>
                        <select className="select" value={pop3.delete_after_days ?? 0} onChange={(e) => setPop3({ ...pop3, delete_after_days: Number(e.target.value) || null })}>
                          <option value={0}>{t("setup.keepForever")}</option>
                          {[7, 14, 30, 90].map((n) => (
                            <option key={n} value={n}>
                              {t("setup.afterDays", { n })}
                            </option>
                          ))}
                        </select>
                      </label>
                    )}
                  </>
                )
              ) : protocol === "imap" ? (
                <>
                  <label className="stack-field">
                    <span className="field-label">{t("setup.username")}</span>
                    <input className="input" value={transport.username} spellCheck={false} onChange={(e) => setTransport({ ...transport, username: e.target.value })} />
                  </label>
                  <label className="stack-field">
                    <span className="field-label">{t("setup.password")}</span>
                    <input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} readOnly={busy} />
                  </label>
                  <ServerFields kind="imap" server={transport.imap} onChange={(imap) => setTransport({ ...transport, imap })} />
                  <ServerFields kind="smtp" server={transport.smtp} onChange={(smtp) => setTransport({ ...transport, smtp })} />
                </>
              ) : (
                <>
                  <label className="stack-field">
                    <span className="field-label">{t("setup.sessionUrl")}</span>
                    <input
                      id="setup-jmap-url"
                      className="input"
                      value={jmapManual.session_url}
                      spellCheck={false}
                      placeholder="https://jmap.example.org/.well-known/jmap"
                      onChange={(e) => setJmapManual({ ...jmapManual, session_url: e.target.value })}
                    />
                  </label>
                  <div className="setup-switch">
                    <Switch label={t("setup.apiToken")} checked={jmapManual.token} onChange={(token) => setJmapManual({ ...jmapManual, token })} />
                    <span>{t("setup.useApiToken")}</span>
                  </div>
                  {!jmapManual.token && (
                    <label className="stack-field">
                      <span className="field-label">{t("setup.username")}</span>
                      <input className="input" value={jmapManual.username} spellCheck={false} onChange={(e) => setJmapManual({ ...jmapManual, username: e.target.value })} />
                    </label>
                  )}
                  <label className="stack-field">
                    <span className="field-label">{jmapManual.token ? t("setup.apiToken") : t("setup.password")}</span>
                    <input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} readOnly={busy} />
                  </label>
                </>
              )}
              {status}
              <div className="setup-actions">
                <button type="button" className="btn" disabled={busy} onClick={() => setManual(false)}>
                  <ArrowLeft size={14} strokeWidth={1.75} aria-hidden />
                  <span>{t("common.back")}</span>
                </button>
                <span className="toolbar-gap" />
                <button
                  type="submit"
                  className="btn btn-primary"
                  disabled={
                    busy ||
                    !password ||
                    (protocol === "imap" ? !transport.imap.host || !transport.smtp.host : protocol === "pop3" ? !pop3?.pop3.host || !pop3.smtp.host : !jmapManual.session_url.trim())
                  }
                >
                  {t("setup.check")}
                </button>
              </div>
            </form>
          )}
        </div>
      </div>
    </div>
  );
}
