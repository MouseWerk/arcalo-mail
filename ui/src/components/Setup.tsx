import { ArrowLeft, CircleCheck, CircleX, Info, KeyRound, Lock, Mail, ServerCog, ShieldCheck, Sparkles } from "lucide-react";
import { useState } from "react";
import { useT, type TKey } from "../lib/i18n";
import { api } from "../lib/api";
import { explain, hintKey } from "../lib/problems";
import type { Account, CheckResult, Discovered, ImapTransport, Problem, Security, Server } from "../lib/types";
import { Spinner } from "./ui";

type Step =
  | { kind: "form" }
  | { kind: "discovering" }
  | { kind: "checking"; found: Discovered | null }
  | { kind: "failed"; result: CheckResult; found: Discovered | null }
  | { kind: "done"; account: Account };

const SOURCE: Record<Discovered["source"], TKey> = {
  preset: "setup.sourcePreset",
  autoconfig: "setup.sourceAutoconfig",
  ispdb: "setup.sourceIspdb",
  srv: "setup.sourceSrv",
  mx: "setup.sourceMx",
  guess: "setup.sourceGuess",
};

const defaultPort = (kind: "imap" | "smtp", s: Security) => (kind === "imap" ? (s === "tls" ? 993 : 143) : s === "tls" ? 465 : 587);

function ServerFields({ kind, server, onChange }: { kind: "imap" | "smtp"; server: Server; onChange: (s: Server) => void }) {
  const t = useT();
  const id = `setup-${kind}`;
  return (
    <fieldset className="server-fields">
      <legend>{kind === "imap" ? t("setup.incoming") : t("setup.outgoing")}</legend>
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

function ProblemCard({ which, p, onTrust }: { which: "imap" | "smtp"; p: Problem; onTrust: () => void }) {
  const t = useT();
  const e = explain(p);
  return (
    <div className="problem" role="alert">
      <CircleX size={18} strokeWidth={1.75} aria-hidden className="problem-icon" />
      <div className="problem-body">
        <p className="problem-title">
          {which === "imap" ? t("setup.imapLabel") : t("setup.smtpLabel")}: {t(e.title, e.vars)}
        </p>
        <p className="problem-hint">{t(e.hint, e.vars)}</p>
        {p.kind === "certificate" && (
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

/** Adding an account: the address and password, the servers found for it, the connection
 *  check with clear errors, and the servers by hand when nothing was found. `welcome`: the first
 *  start, with a short introduction. */
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

  const check = async (tr: ImapTransport, d: Discovered | null) => {
    setTransport(tr);
    setStep({ kind: "checking", found: d });
    setError(null);
    try {
      const result = await api.check(tr, password);
      if (result.imap || result.smtp || result.error) {
        setStep({ kind: "failed", result, found: d });
        return;
      }
      const account = await api.addAccount({ provider: "imap", name: "", email: email.trim(), sender_name: name.trim(), transport: tr }, password);
      setStep({ kind: "done", account });
    } catch (e) {
      setError(String(e));
      setStep({ kind: "form" });
    }
  };

  const start = async () => {
    if (!email.trim() || !password) return;
    setError(null);
    setStep({ kind: "discovering" });
    try {
      const d = await api.discover(email.trim());
      setFound(d);
      if (d.hint === "microsoft_sign_in") {
        setStep({ kind: "form" });
        setError(t("setup.hintMicrosoft"));
        return;
      }
      await check({ kind: "imap", imap: d.imap, smtp: d.smtp, username: d.username, auth: { kind: "password" } }, d);
    } catch (e) {
      setError(String(e));
      setStep({ kind: "form" });
    }
  };

  const trust = (which: "imap" | "smtp", p: Problem) => {
    if (!transport || p.kind !== "certificate") return;
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
          <button type="button" className="btn btn-primary btn-lg" autoFocus onClick={() => onDone(step.account)}>
            {t("setup.toInbox")}
          </button>
        </div>
      </div>
    );

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
          {step.result.imap && <ProblemCard which="imap" p={step.result.imap} onTrust={() => step.result.imap && trust("imap", step.result.imap)} />}
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
            {t("setup.found", { imap: found.imap.host, smtp: found.smtp.host })} {t(SOURCE[found.source], { provider: found.provider ?? "" })}
          </span>
        </p>
      )}
    </>
  );
  return (
    <div className={`setup${welcome ? " setup-welcome" : ""}`}>
      <div className="setup-card">
        {welcome && (
          <div className="setup-intro">
            <span className="setup-logo" aria-hidden>
              <Mail size={22} strokeWidth={1.75} />
            </span>
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
        )}
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
              <label className="stack-field">
                <span className="field-label">{t("setup.password")}</span>
                <input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} readOnly={busy} autoComplete="current-password" />
              </label>
              <p className="setup-note muted">
                <KeyRound size={13} strokeWidth={1.75} aria-hidden />
                <span>{t("setup.passwordNote")}</span>
              </p>
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
                    const domain = email.split("@")[1] ?? "";
                    setTransport(
                      transport ?? {
                        kind: "imap",
                        imap: found?.imap ?? { host: domain ? `imap.${domain}` : "", port: 993, security: "tls" },
                        smtp: found?.smtp ?? { host: domain ? `smtp.${domain}` : "", port: 587, security: "start_tls" },
                        username: found?.username ?? email.trim(),
                        auth: { kind: "password" },
                      },
                    );
                    setManual(true);
                  }}
                >
                  {t("setup.manual")}
                </button>
                <span className="toolbar-gap" />
                <button type="submit" className="btn btn-primary" disabled={busy || !email.trim() || !password}>
                  {step.kind === "failed" ? t("setup.tryAgain") : t("setup.continue")}
                </button>
              </div>
            </form>
          )}
          {manual && transport && (
            <form
              className="setup-fields"
              onSubmit={(e) => {
                e.preventDefault();
                void check(transport, found);
              }}
            >
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
              {status}
              <div className="setup-actions">
                <button type="button" className="btn" disabled={busy} onClick={() => setManual(false)}>
                  <ArrowLeft size={14} strokeWidth={1.75} aria-hidden />
                  <span>{t("common.back")}</span>
                </button>
                <span className="toolbar-gap" />
                <button type="submit" className="btn btn-primary" disabled={busy || !transport.imap.host || !transport.smtp.host || !password}>
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
