import { Bell, ChevronLeft, ChevronRight, Info, Keyboard, LogIn, Network, Palette, PenLine, Shield, Trash2, UserRound, X, Plus } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { useT, type TKey } from "../lib/i18n";
import { api, pickSavePath } from "../lib/api";
import { showToast } from "../lib/toasts";
import type { AccountInfo, AppInfo, ImapTransport, LanguageChoice, MicrosoftSignIn, NetworkSettings, Prefetch, ProxyMode, ProxyProfile, RouteGroup, Security, Server, Settings as SettingsT, SwipeAction, SystemProxy, ThemeChoice } from "../lib/types";
import { explain } from "../lib/problems";
import { ConfirmDialog, Segmented, Spinner, Switch, trapFocus } from "./ui";
import { ProblemCard, Setup } from "./Setup";
import { useRef } from "react";

export type SettingsSection = "accounts" | "appearance" | "reading" | "writing" | "notifications" | "privacy" | "network" | "shortcuts" | "about";

const SECTIONS: [SettingsSection, TKey, typeof Bell][] = [
  ["accounts", "settings.accounts", UserRound],
  ["appearance", "settings.appearance", Palette],
  ["reading", "settings.reading", Info],
  ["writing", "settings.writing", PenLine],
  ["notifications", "settings.notifications", Bell],
  ["privacy", "settings.privacy", Shield],
  ["network", "settings.network", Network],
  ["shortcuts", "settings.shortcuts", Keyboard],
  ["about", "settings.about", Info],
];

const SHORTCUTS: [string, TKey][] = [
  ["j / k", "keys.nextPrev"],
  ["Enter", "keys.open"],
  ["e", "keys.archive"],
  ["#", "keys.delete"],
  ["!", "keys.spam"],
  ["r", "keys.reply"],
  ["a", "keys.replyAll"],
  ["f", "keys.forward"],
  ["c", "keys.compose"],
  ["/", "keys.search"],
  ["u", "keys.undo"],
  ["x", "keys.select"],
  ["s", "keys.flag"],
  ["Shift+I / Shift+U", "keys.readUnread"],
  ["Ctrl+Enter", "keys.send"],
  ["Ctrl+,", "keys.settings"],
  ["Ctrl+K", "keys.palette"],
];

/** Asks where to save the diagnostics report and writes it. */
export async function exportDiagnostics(t: ReturnType<typeof useT>): Promise<void> {
  const name = `arcalo-mail-diagnose-${new Date().toISOString().slice(0, 10)}.txt`;
  const path = await pickSavePath(name);
  if (!path) return;
  try {
    await api.exportDiagnostics(path);
    showToast({ title: t("toast.diagnosticsSaved", { name: path.split(/[\\/]/).pop() ?? name }), duration: 4000, tone: "success" });
  } catch (e) {
    showToast({ title: String(e), tone: "danger" });
  }
}

function Row({ label, description, children }: { label: string; description?: string; children: ReactNode }) {
  return (
    <div className="set-row">
      <div className="set-text">
        <div className="set-label">{label}</div>
        {description && <div className="set-desc">{description}</div>}
      </div>
      <div className="set-control">{children}</div>
    </div>
  );
}

function ServerEdit({ label, server, onChange }: { label: string; server: Server; onChange: (s: Server) => void }) {
  const t = useT();
  return (
    <fieldset className="server-fields">
      <legend>{label}</legend>
      <label className="stack-field server-host">
        <span className="field-label">{t("setup.host")}</span>
        <input className="input" value={server.host} spellCheck={false} onChange={(e) => onChange({ ...server, host: e.target.value.trim() })} />
      </label>
      <label className="stack-field server-port">
        <span className="field-label">{t("setup.port")}</span>
        <input className="input" inputMode="numeric" value={server.port || ""} onChange={(e) => onChange({ ...server, port: Number(e.target.value.replace(/\D/g, "")) || 0 })} />
      </label>
      <label className="stack-field server-security">
        <span className="field-label">{t("setup.security")}</span>
        <select className="select" value={server.security} onChange={(e) => onChange({ ...server, security: e.target.value as Security })}>
          <option value="tls">{t("setup.tls")}</option>
          <option value="start_tls">{t("setup.starttls")}</option>
          <option value="plain">{t("setup.plain")}</option>
        </select>
      </label>
    </fieldset>
  );
}

function AccountEditor({ a, onChanged, onRemoved }: { a: AccountInfo; onChanged: () => void; onRemoved: () => void }) {
  const t = useT();
  const [name, setName] = useState(a.name);
  const [sender, setSender] = useState(a.sender_name);
  const [signature, setSignature] = useState(a.signature);
  const [transport, setTransport] = useState<ImapTransport | null>(a.transport.kind === "imap" ? a.transport : null);
  const [password, setPassword] = useState("");
  const [state, setState] = useState<{ busy: boolean; message: string | null; ok: boolean }>({ busy: false, message: null, ok: false });
  const [confirm, setConfirm] = useState(false);

  useEffect(() => {
    setName(a.name);
    setSender(a.sender_name);
    setSignature(a.signature);
    setTransport(a.transport.kind === "imap" ? a.transport : null);
  }, [a.id]);

  const saveProfile = async () => {
    await api.updateAccount(a.id, name, sender, signature);
    onChanged();
  };

  const saveServers = async () => {
    if (!transport) return;
    setState({ busy: true, message: null, ok: false });
    try {
      if (password) {
        const r = await api.check(transport, password);
        const p = r.imap ?? r.smtp;
        if (p || r.error) {
          const e = p ? explain(p) : null;
          setState({ busy: false, message: e ? `${t(e.title, e.vars)} – ${t(e.hint, e.vars)}` : (r.error ?? ""), ok: false });
          return;
        }
      }
      await api.updateServers(a.id, transport, password || null);
      setPassword("");
      setState({ busy: false, message: t("settings.serversSaved"), ok: true });
      onChanged();
    } catch (e) {
      setState({ busy: false, message: String(e), ok: false });
    }
  };

  return (
    <div className="set-section">
      <div className="set-card">
        <Row label={t("settings.accountName")} description={t("settings.accountNameHint")}>
          <input className="input" value={name} placeholder={a.email} onChange={(e) => setName(e.target.value)} onBlur={() => void saveProfile()} />
        </Row>
        <Row label={t("settings.senderName")} description={t("settings.senderNameHint")}>
          <input className="input" value={sender} onChange={(e) => setSender(e.target.value)} onBlur={() => void saveProfile()} />
        </Row>
        <div className="set-row set-row-stack">
          <div className="set-text">
            <div className="set-label">{t("settings.signature")}</div>
            <div className="set-desc">{t("settings.signatureHint")}</div>
          </div>
          <textarea className="input textarea" rows={4} value={signature} onChange={(e) => setSignature(e.target.value)} onBlur={() => void saveProfile()} />
        </div>
      </div>
      {a.transport.kind === "imap" && a.transport.auth.kind === "o_auth2" && <MicrosoftSignInRow a={a} />}
      {a.transport.kind === "jmap" && <JmapRow a={a} />}
      {transport && transport.auth.kind === "password" && (
        <div className="set-card">
          <div className="set-card-title">{t("settings.servers")}</div>
          <label className="stack-field">
            <span className="field-label">{t("setup.username")}</span>
            <input className="input" value={transport.username} spellCheck={false} onChange={(e) => setTransport({ ...transport, username: e.target.value })} />
          </label>
          <ServerEdit label={t("setup.incoming")} server={transport.imap} onChange={(imap) => setTransport({ ...transport, imap })} />
          <ServerEdit label={t("setup.outgoing")} server={transport.smtp} onChange={(smtp) => setTransport({ ...transport, smtp })} />
          <label className="stack-field">
            <span className="field-label">{t("settings.newPassword")}</span>
            <input className="input" type="password" value={password} placeholder={t("settings.newPasswordHint")} onChange={(e) => setPassword(e.target.value)} />
          </label>
          {state.message && (
            <p className={state.ok ? "field-ok" : "field-error"} role={state.ok ? "status" : "alert"}>
              {state.message}
            </p>
          )}
          <div className="set-actions">
            <button type="button" className="btn btn-primary" disabled={state.busy} onClick={() => void saveServers()}>
              {state.busy && <Spinner />}
              <span>{password ? t("settings.checkAndSave") : t("settings.save")}</span>
            </button>
          </div>
        </div>
      )}
      <div className="set-card set-card-danger">
        <Row label={t("settings.removeAccount")} description={t("settings.removeAccountHint")}>
          <button type="button" className="btn btn-danger-quiet" onClick={() => setConfirm(true)}>
            <Trash2 size={14} strokeWidth={1.75} aria-hidden />
            <span>{t("settings.remove")}</span>
          </button>
        </Row>
      </div>
      {confirm && (
        <ConfirmDialog
          title={t("settings.removeTitle", { email: a.email })}
          text={t("settings.removeText")}
          confirm={t("settings.remove")}
          danger
          onConfirm={() => void api.removeAccount(a.id).then(onRemoved)}
          onClose={() => setConfirm(false)}
        />
      )}
    </div>
  );
}

/** A Microsoft account: no servers to edit; signing in again in the browser. */
function MicrosoftSignInRow({ a }: { a: AccountInfo }) {
  const t = useT();
  const [state, setState] = useState<{ busy: boolean; result: MicrosoftSignIn | null }>({ busy: false, result: null });
  const r = state.result;
  return (
    <div className="set-card">
      <Row label={t("settings.microsoftAccount")} description={t("settings.microsoftAccountHint")}>
        <button
          type="button"
          className="btn"
          disabled={state.busy}
          onClick={() => {
            setState({ busy: true, result: null });
            void api.microsoftReauth(a.id).then(
              (result) => setState({ busy: false, result }),
              (e) => setState({ busy: false, result: { session: null, email: null, check: null, problem: null, error: String(e) } }),
            );
          }}
        >
          {state.busy ? <Spinner /> : <LogIn size={14} strokeWidth={1.75} aria-hidden />}
          <span>{t("settings.signInAgain")}</span>
        </button>
      </Row>
      {r?.problem && <ProblemCard which="microsoft" p={r.problem} />}
      {r?.error && (
        <p className="field-error" role="alert">
          {r.error}
        </p>
      )}
    </div>
  );
}

/** A JMAP account: its address and a new password or token. */
function JmapRow({ a }: { a: AccountInfo }) {
  const t = useT();
  const [password, setPassword] = useState("");
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  if (a.transport.kind !== "jmap") return null;
  const tr = a.transport;
  const save = async () => {
    const r = await api.checkJmap(tr.session_url, tr.username, password, tr.auth.kind === "token");
    const p = r.imap;
    if (p || r.error) {
      const e = p ? explain(p) : null;
      setMessage({ ok: false, text: e ? `${t(e.title, e.vars)} – ${t(e.hint, e.vars)}` : (r.error ?? "") });
      return;
    }
    await api.signIn(a.id, password);
    setPassword("");
    setMessage({ ok: true, text: t("settings.serversSaved") });
  };
  return (
    <div className="set-card">
      <div className="set-card-title">{t("settings.servers")}</div>
      <Row label={t("settings.jmapServer")}>
        <code className="set-code">{tr.session_url}</code>
      </Row>
      <label className="stack-field">
        <span className="field-label">{tr.auth.kind === "token" ? t("setup.apiToken") : t("settings.newPassword")}</span>
        <input className="input" type="password" value={password} placeholder={t("settings.newPasswordHint")} onChange={(e) => setPassword(e.target.value)} />
      </label>
      {message && (
        <p className={message.ok ? "field-ok" : "field-error"} role={message.ok ? "status" : "alert"}>
          {message.text}
        </p>
      )}
      <div className="set-actions">
        <button type="button" className="btn btn-primary" disabled={!password} onClick={() => void save()}>
          {t("settings.checkAndSave")}
        </button>
      </div>
    </div>
  );
}

const ROUTES: [RouteGroup, TKey][] = [
  ["mail", "net.routeMail"],
  ["sign_in", "net.routeSignIn"],
  ["setup", "net.routeSetup"],
  ["updates", "net.routeUpdates"],
];

/** Settings → Netzwerk: proxy profiles as in Arcalo, and which connection uses which. */
function NetworkSection({ net, onChange }: { net: NetworkSettings; onChange: (n: NetworkSettings) => void }) {
  const t = useT();
  const [system, setSystem] = useState<SystemProxy | null>(null);
  const [draft, setDraft] = useState<NetworkSettings>(net);
  const [passwords, setPasswords] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  useEffect(() => setDraft(net), [net]);
  useEffect(() => {
    void api.systemProxy().then(setSystem);
  }, []);
  const commit = (n: NetworkSettings) => {
    const manualWithout = n.profiles.find((p) => p.mode === "manual" && !p.http_proxy.trim() && !p.socks_proxy.trim());
    setDraft(n);
    if (manualWithout) return;
    setError(null);
    onChange(n);
  };
  const setProfile = (i: number, patch: Partial<ProxyProfile>, save: boolean) => {
    const next = { ...draft, profiles: draft.profiles.map((p, j) => (j === i ? { ...p, ...patch } : p)) };
    if (save) commit(next);
    else setDraft(next);
  };
  const add = () => {
    let n = draft.profiles.length;
    const ids = new Set(draft.profiles.map((p) => p.id));
    while (ids.has(`profil-${n}`)) n++;
    commit({
      ...draft,
      profiles: [...draft.profiles, { id: `profil-${n}`, name: t("net.newName", { n }), mode: "manual", http_proxy: "", socks_proxy: "", no_proxy: "localhost, 127.0.0.1, ::1", proxy_user: "" }],
    });
  };
  const remove = (id: string) => {
    const routes = Object.fromEntries(Object.entries(draft.routes).filter(([, v]) => v !== id));
    commit({ profiles: draft.profiles.filter((p) => p.id !== id), routes });
  };
  const sys = system && (system.http || system.https || system.socks);
  return (
    <div className="set-section">
      <p className="muted set-intro">{t("net.profilesHint")}</p>
      {draft.profiles.map((p, i) => (
        <div className="set-card" key={p.id}>
          <div className="set-card-head">
            <input
              className="input set-card-name"
              aria-label={t("net.name")}
              value={p.name}
              disabled={p.id === "standard"}
              onChange={(e) => setProfile(i, { name: e.target.value }, false)}
              onBlur={() => commit(draft)}
            />
            {p.id !== "standard" && (
              <button type="button" className="btn btn-danger-quiet btn-sm" onClick={() => remove(p.id)}>
                <Trash2 size={14} strokeWidth={1.75} aria-hidden />
                <span>{t("net.remove")}</span>
              </button>
            )}
          </div>
          <Row label={t("net.mode")}>
            <Segmented<ProxyMode>
              label={t("net.mode")}
              value={p.mode}
              onChange={(mode) => setProfile(i, { mode }, true)}
              options={[
                ["none", t("net.modeNone")],
                ["system", t("net.modeSystem")],
                ["manual", t("net.modeManual")],
              ]}
            />
          </Row>
          {p.mode === "system" && (
            <div className="set-row set-row-stack">
              <div className="set-text">
                <div className="set-label">{t("net.system")}</div>
                <div className="set-desc">
                  {sys ? `${[system?.https ?? system?.http, system?.socks].filter(Boolean).join(", ")} – ${t("net.systemFrom", { source: system?.source ?? "" })}` : t("net.systemNone")}
                </div>
                <div className="set-desc">{t("net.systemMail")}</div>
              </div>
            </div>
          )}
          {p.mode === "manual" && (
            <div className="set-fields">
              <label className="stack-field">
                <span className="field-label">{t("net.httpProxy")}</span>
                <input className="input" value={p.http_proxy} placeholder={t("net.proxyPlaceholder")} spellCheck={false} onChange={(e) => setProfile(i, { http_proxy: e.target.value }, false)} onBlur={() => commit(draft)} />
              </label>
              <label className="stack-field">
                <span className="field-label">{t("net.socksProxy")}</span>
                <input className="input" value={p.socks_proxy} placeholder={t("net.proxyPlaceholder")} spellCheck={false} onChange={(e) => setProfile(i, { socks_proxy: e.target.value }, false)} onBlur={() => commit(draft)} />
              </label>
              <label className="stack-field">
                <span className="field-label">{t("net.noProxy")}</span>
                <input className="input" value={p.no_proxy} spellCheck={false} onChange={(e) => setProfile(i, { no_proxy: e.target.value }, false)} onBlur={() => commit(draft)} />
                <span className="field-hint">{t("net.noProxyHint")}</span>
              </label>
              <label className="stack-field">
                <span className="field-label">{t("net.user")}</span>
                <input className="input" value={p.proxy_user} spellCheck={false} onChange={(e) => setProfile(i, { proxy_user: e.target.value }, false)} onBlur={() => commit(draft)} />
              </label>
              {p.proxy_user.trim() && (
                <label className="stack-field">
                  <span className="field-label">{t("net.password")}</span>
                  <input
                    className="input"
                    type="password"
                    value={passwords[p.id] ?? ""}
                    onChange={(e) => setPasswords({ ...passwords, [p.id]: e.target.value })}
                    onBlur={() => {
                      const pw = passwords[p.id];
                      if (pw === undefined) return;
                      void api.setProxyPassword(p.id, pw).then(
                        () => showToast({ title: t("net.passwordSaved"), duration: 2500, tone: "success" }),
                        (e) => setError(String(e)),
                      );
                    }}
                  />
                  <span className="field-hint">{t("net.passwordHint")}</span>
                </label>
              )}
            </div>
          )}
        </div>
      ))}
      <div className="set-actions set-actions-left">
        <button type="button" className="btn" onClick={add}>
          <Plus size={14} strokeWidth={1.75} aria-hidden />
          <span>{t("net.add")}</span>
        </button>
      </div>
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
      <div className="set-card">
        <div className="set-card-title">{t("net.routes")}</div>
        {ROUTES.map(([group, label]) => (
          <Row key={group} label={t(label)}>
            <select
              className="select"
              aria-label={t(label)}
              value={draft.routes[group] ?? "standard"}
              onChange={(e) => {
                const routes = { ...draft.routes };
                if (e.target.value === "standard") delete routes[group];
                else routes[group] = e.target.value;
                commit({ ...draft, routes });
              }}
            >
              {draft.profiles.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
          </Row>
        ))}
      </div>
    </div>
  );
}

type Props = {
  settings: SettingsT;
  accounts: AccountInfo[];
  info: AppInfo | null;
  section: SettingsSection;
  onSection: (s: SettingsSection) => void;
  onChange: (patch: Partial<SettingsT>) => void;
  onAccountsChanged: () => void;
  onClose: () => void;
  /** Touch screens: the swipe settings. */
  touch?: boolean;
  /** Phones: the list of sections is a page of its own (`nav`), a section opens over it. */
  phone?: { nav: boolean; onNav: (open: boolean) => void };
};

const SWIPES: [SwipeAction, TKey][] = [
  ["archive", "read.archive"],
  ["delete", "read.delete"],
  ["read", "settings.swipeRead"],
  ["flag", "read.flag"],
  ["none", "settings.swipeNone"],
];

export function SettingsView({ settings, accounts, info, section, onSection, onChange, onAccountsChanged, onClose, touch, phone }: Props) {
  const t = useT();
  const mobile = info?.platform === "android" || info?.platform === "ios";
  const [accountId, setAccountId] = useState<number | null>(accounts[0]?.id ?? null);
  const [adding, setAdding] = useState(false);
  const [senders, setSenders] = useState<string[]>([]);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (section === "privacy") void api.allowedSenders().then(setSenders);
  }, [section]);
  useEffect(() => {
    if (accountId == null || !accounts.some((a) => a.id === accountId)) setAccountId(accounts[0]?.id ?? null);
  }, [accounts]);
  useEffect(() => {
    ref.current?.querySelector<HTMLElement>(".settings-nav-item[aria-current]")?.focus();
  }, []);

  const account = accounts.find((a) => a.id === accountId) ?? null;
  const store = info?.secret_store;

  return (
    <div className="overlay settings-overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        ref={ref}
        className="settings"
        data-page={phone ? (phone.nav ? "nav" : "section") : undefined}
        role="dialog"
        aria-modal="true"
        aria-label={t("settings.title")}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onClose();
          }
          if (e.key === "Tab") trapFocus(e, ref.current);
        }}
      >
        <nav className="settings-nav" aria-label={t("settings.sections")}>
          <div className="settings-nav-title">
            <span>{t("settings.title")}</span>
            {phone && (
              <button type="button" className="icon-btn" onClick={onClose} aria-label={t("common.close")}>
                <X size={18} strokeWidth={1.75} aria-hidden />
              </button>
            )}
          </div>
          {SECTIONS.map(([id, label, Icon]) => (
            <button
              key={id}
              type="button"
              className="settings-nav-item"
              aria-current={section === id && !phone ? "page" : undefined}
              onClick={() => {
                onSection(id);
                phone?.onNav(false);
              }}
            >
              <Icon size={15} strokeWidth={1.75} aria-hidden />
              <span>{t(label)}</span>
              {phone && <ChevronRight size={16} strokeWidth={1.75} aria-hidden className="settings-nav-chevron" />}
            </button>
          ))}
        </nav>
        <div className="settings-main">
          <header className="settings-head">
            {phone && (
              <button type="button" className="icon-btn" onClick={() => phone.onNav(true)} aria-label={t("common.back")}>
                <ChevronLeft size={20} strokeWidth={1.75} aria-hidden />
              </button>
            )}
            <h2>{t(SECTIONS.find(([id]) => id === section)?.[1] ?? "settings.title")}</h2>
            <button type="button" className="icon-btn" onClick={onClose} aria-label={t("common.close")}>
              <X size={16} strokeWidth={1.75} aria-hidden />
            </button>
          </header>
          <div className="settings-body">
            {section === "accounts" &&
              (adding ? (
                <Setup
                  welcome={false}
                  onCancel={() => setAdding(false)}
                  onDone={(a) => {
                    setAdding(false);
                    setAccountId(a.id);
                    onAccountsChanged();
                  }}
                />
              ) : (
                <>
                  <div className="account-tabs" role="tablist" aria-label={t("settings.accounts")}>
                    {accounts.map((a) => (
                      <button key={a.id} type="button" role="tab" aria-selected={a.id === accountId} className="account-tab" onClick={() => setAccountId(a.id)}>
                        <span className="account-tab-name">{a.name || a.email}</span>
                        {a.name && <span className="account-tab-email">{a.email}</span>}
                      </button>
                    ))}
                    <button type="button" className="account-tab account-add" onClick={() => setAdding(true)}>
                      <Plus size={14} strokeWidth={1.75} aria-hidden />
                      <span>{t("settings.addAccount")}</span>
                    </button>
                  </div>
                  {account ? (
                    <AccountEditor a={account} onChanged={onAccountsChanged} onRemoved={onAccountsChanged} />
                  ) : (
                    <p className="muted">{t("nav.noAccount")}</p>
                  )}
                </>
              ))}
            {section === "appearance" && (
              <div className="set-card">
                <Row label={t("theme.label")}>
                  <Segmented<ThemeChoice>
                    label={t("theme.label")}
                    value={settings.theme}
                    onChange={(theme) => onChange({ theme })}
                    options={[
                      ["system", t("theme.system")],
                      ["light", t("theme.light")],
                      ["dark", t("theme.dark")],
                      ["contrast", t("theme.contrast")],
                    ]}
                  />
                </Row>
                <Row label={t("lang.label")}>
                  <select className="select" value={settings.language} onChange={(e) => onChange({ language: e.target.value as LanguageChoice })} aria-label={t("lang.label")}>
                    <option value="system">{t("theme.system")}</option>
                    <option value="de">{t("lang.de")}</option>
                    <option value="en">{t("lang.en")}</option>
                  </select>
                </Row>
                <Row label={t("settings.threads")} description={t("settings.threadsHint")}>
                  <Switch label={t("settings.threads")} checked={settings.threads} onChange={(threads) => onChange({ threads })} />
                </Row>
                <Row label={t("settings.subjectThreads")} description={t("settings.subjectThreadsHint")}>
                  <Switch label={t("settings.subjectThreads")} checked={settings.subject_threads} onChange={(subject_threads) => onChange({ subject_threads })} />
                </Row>
                <Row label={t("settings.snippets")} description={t("settings.snippetsHint")}>
                  <Switch label={t("settings.snippets")} checked={settings.show_snippets} onChange={(show_snippets) => onChange({ show_snippets })} />
                </Row>
              </div>
            )}
            {section === "reading" && (
              <div className="set-card">
                <Row label={t("settings.markRead")} description={t("settings.markReadHint")}>
                  <select
                    className="select"
                    aria-label={t("settings.markRead")}
                    value={settings.mark_read_after_secs == null ? "never" : String(settings.mark_read_after_secs)}
                    onChange={(e) => onChange({ mark_read_after_secs: e.target.value === "never" ? null : Number(e.target.value) })}
                  >
                    <option value="0">{t("settings.markReadNow")}</option>
                    <option value="2">{t("settings.markReadAfter", { n: 2 })}</option>
                    <option value="never">{t("settings.markReadNever")}</option>
                  </select>
                </Row>
                <Row label={t("settings.prefetch")} description={t("settings.prefetchHint")}>
                  <select className="select" aria-label={t("settings.prefetch")} value={settings.prefetch} onChange={(e) => onChange({ prefetch: e.target.value as Prefetch })}>
                    <option value="none">{t("settings.prefetchNone")}</option>
                    <option value="recent">{t("settings.prefetchRecent")}</option>
                    <option value="month">{t("settings.prefetchMonth")}</option>
                  </select>
                </Row>
              </div>
            )}
            {section === "reading" && touch && (
              <div className="set-card" aria-label={t("settings.gestures")} role="group">
                {(["swipe_left", "swipe_right"] as const).map((key) => (
                  <Row key={key} label={t(key === "swipe_left" ? "settings.swipeLeft" : "settings.swipeRight")} description={t("settings.swipeHint")}>
                    <select
                      className="select"
                      aria-label={t(key === "swipe_left" ? "settings.swipeLeft" : "settings.swipeRight")}
                      value={settings[key]}
                      onChange={(e) => onChange({ [key]: e.target.value as SwipeAction })}
                    >
                      {SWIPES.map(([value, label]) => (
                        <option key={value} value={value}>
                          {t(label)}
                        </option>
                      ))}
                    </select>
                  </Row>
                ))}
              </div>
            )}
            {section === "network" && <NetworkSection net={settings.network} onChange={(network) => onChange({ network })} />}
            {section === "writing" && (
              <div className="set-card">
                <Row label={t("settings.richText")} description={t("settings.richTextHint")}>
                  <Switch label={t("settings.richText")} checked={settings.compose_rich_text} onChange={(compose_rich_text) => onChange({ compose_rich_text })} />
                </Row>
                <Row label={t("settings.undoSend")} description={t("settings.undoSendHint")}>
                  <select className="select" aria-label={t("settings.undoSend")} value={String(settings.undo_send_secs)} onChange={(e) => onChange({ undo_send_secs: Number(e.target.value) })}>
                    <option value="0">{t("settings.undoSendOff")}</option>
                    {[5, 10, 20, 30].map((n) => (
                      <option key={n} value={n}>
                        {t("settings.seconds", { n })}
                      </option>
                    ))}
                  </select>
                </Row>
              </div>
            )}
            {section === "notifications" && (
              <div className="set-card">
                <Row label={t("settings.notify")} description={t("settings.notifyHint")}>
                  <Switch label={t("settings.notify")} checked={settings.notifications} onChange={(notifications) => onChange({ notifications })} />
                </Row>
                {mobile && (
                  <Row label={t("settings.backgroundCheck")} description={t("settings.backgroundCheckHint")}>
                    <Switch label={t("settings.backgroundCheck")} checked={settings.background_check} onChange={(background_check) => onChange({ background_check })} />
                  </Row>
                )}
              </div>
            )}
            {section === "privacy" && (
              <>
                <div className="set-card">
                  <Row label={t("settings.remote")} description={t("settings.remoteHint")}>
                    <Segmented
                      label={t("settings.remote")}
                      value={settings.remote_content === "always" ? "always" : "ask"}
                      onChange={(v) => onChange({ remote_content: v })}
                      options={[
                        ["ask", t("settings.remoteAsk")],
                        ["always", t("settings.remoteAlways")],
                      ]}
                    />
                  </Row>
                  <div className="set-row set-row-stack">
                    <div className="set-text">
                      <div className="set-label">{t("settings.trustedSenders")}</div>
                      <div className="set-desc">{t("settings.trustedSendersHint")}</div>
                    </div>
                    {senders.length === 0 ? (
                      <p className="muted set-empty">{t("settings.noTrustedSenders")}</p>
                    ) : (
                      <ul className="sender-list">
                        {senders.map((s) => (
                          <li key={s}>
                            <span>{s}</span>
                            <button type="button" className="icon-btn icon-btn-sm" aria-label={t("settings.forgetSender", { email: s })} onClick={() => void api.forbidSender(s).then(() => setSenders(senders.filter((x) => x !== s)))}>
                              <X size={14} strokeWidth={1.75} aria-hidden />
                            </button>
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>
                </div>
                <div className="set-card">
                  <Row
                    label={t("settings.passwords")}
                    description={
                      store?.kind === "file" ? t("settings.passwordsFile", { reason: store.reason }) : store?.kind === "keystore" ? t("settings.passwordsKeystore") : t("settings.passwordsSystem")
                    }
                  >
                    <span className="set-badge">{store?.kind === "file" ? t("settings.storeFile") : store?.kind === "keystore" ? t("settings.storeKeystore") : t("settings.storeSystem")}</span>
                  </Row>
                  <Row label={t("settings.localFirst")} description={t("settings.localFirstHint")}>
                    <span />
                  </Row>
                </div>
              </>
            )}
            {section === "shortcuts" && (
              <div className="set-card">
                <table className="keys">
                  <tbody>
                    {SHORTCUTS.map(([k, label]) => (
                      <tr key={k}>
                        <td>
                          {k.split(" / ").map((part, i) => (
                            <span key={part}>
                              {i > 0 && " / "}
                              <kbd>{part.replace("Ctrl", t("keys.ctrl")).replace("Shift", t("keys.shift")).replace("Enter", t("keys.enter"))}</kbd>
                            </span>
                          ))}
                        </td>
                        <td>{t(label)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
            {section === "about" && (
              <div className="set-card">
                <Row label={t("app.name")} description={t("settings.aboutText")}>
                  <span className="set-badge">{info?.version ?? ""}</span>
                </Row>
                <Row label={t("settings.license")} description={t("settings.licenseText")}>
                  <span />
                </Row>
                <Row label={t("settings.diagnostics")} description={t("settings.diagnosticsHint")}>
                  <button type="button" className="btn" onClick={() => void exportDiagnostics(t)}>
                    {t("settings.exportDiagnostics")}
                  </button>
                </Row>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
