import { Bell, BookUser, CalendarDays, KeyRound, ChevronLeft, ChevronRight, FileText, Inbox, Info, Keyboard, Lock, LogIn, Network, Palette, PenLine, Shield, Sparkles, Trash2, UserRound, Workflow, X, Plus } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { useT, type TKey } from "../lib/i18n";
import { api, pickSavePath } from "../lib/api";
import { showToast } from "../lib/toasts";
import type { DefaultMail } from "../lib/types";
import { UpdateSettings } from "./Updates";
import type { AccountInfo, AppInfo, ImapTransport, LanguageChoice, MicrosoftSignIn, NetworkSettings, Pop3Transport, Prefetch, ProxyMode, ProxyProfile, RouteGroup, Security, Server, Settings as SettingsT, SwipeAction, SystemProxy, ThemeChoice } from "../lib/types";
import { AiSection, ContactsSection, RulesSection, TemplatesSection } from "./PowerSettings";
import { CalendarSection, CryptoSection } from "./CalendarSettings";
import { SecuritySection } from "./Security";
import { SyncDetails } from "./SyncDetails";
import { explain } from "../lib/problems";
import { SHORTCUTS, keyLabel, usesCommand } from "../lib/shortcuts";
import { ConfirmDialog, Segmented, Select, Spinner, Switch, trapFocus, useSaveOnLeave } from "./ui";
import { ProblemCard, Setup } from "./Setup";
import { useRef } from "react";

export type SettingsSection =
  | "accounts"
  | "appearance"
  | "reading"
  | "inbox"
  | "writing"
  | "templates"
  | "rules"
  | "contacts"
  | "calendar"
  | "encryption"
  | "ai"
  | "notifications"
  | "privacy"
  | "security"
  | "network"
  | "shortcuts"
  | "about";

const SECTIONS: [SettingsSection, TKey, typeof Bell][] = [
  ["accounts", "settings.accounts", UserRound],
  ["appearance", "settings.appearance", Palette],
  ["reading", "settings.reading", Info],
  ["inbox", "settings.inbox", Inbox],
  ["writing", "settings.writing", PenLine],
  ["templates", "settings.templates", FileText],
  ["rules", "settings.rules", Workflow],
  ["contacts", "settings.contacts", BookUser],
  ["calendar", "settings.calendar", CalendarDays],
  ["encryption", "settings.encryption", KeyRound],
  ["ai", "settings.ai", Sparkles],
  ["notifications", "settings.notifications", Bell],
  ["privacy", "settings.privacy", Shield],
  ["security", "settings.security", Lock],
  ["network", "settings.network", Network],
  ["shortcuts", "settings.shortcuts", Keyboard],
  ["about", "settings.about", Info],
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

/** „Als Standard-Mail-App festlegen“ where the system lets an app ask (src-tauri/src/defaultmail.rs). */
function DefaultMailCard() {
  const t = useT();
  const [dm, setDm] = useState<DefaultMail | null>(null);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    void api.defaultMailStatus().then(setDm, () => setDm(null));
  }, []);
  if (!dm || (!dm.supported && !dm.note)) return null;
  const set = async () => {
    setBusy(true);
    try {
      const after = await api.setDefaultMail();
      setDm(after);
      if (after.how === "settings") showToast({ title: t("dm.chooseInSettings") });
    } catch (e) {
      showToast({ title: t("dm.setFailed"), detail: String(e), tone: "danger" });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="set-card" data-default-mail>
      <Row label={t("dm.label")} description={dm.note ?? (dm.is_default ? t("dm.isDefault") : t("dm.hint"))}>
        {dm.supported && !dm.is_default && (
          <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void set()}>
            {busy && <Spinner />}
            <span>{t("dm.set")}</span>
          </button>
        )}
        {dm.is_default && <span className="set-badge">{t("dm.default")}</span>}
      </Row>
    </div>
  );
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
        <input className="input" value={server.host} spellCheck={false} autoCapitalize="off" autoCorrect="off" onChange={(e) => onChange({ ...server, host: e.target.value.trim() })} />
      </label>
      <label className="stack-field server-port">
        <span className="field-label">{t("setup.port")}</span>
        <input className="input" inputMode="numeric" value={server.port || ""} onChange={(e) => onChange({ ...server, port: Number(e.target.value.replace(/\D/g, "")) || 0 })} />
      </label>
      <label className="stack-field server-security">
        <span className="field-label">{t("setup.security")}</span>
        <Select className="select" value={server.security} onChange={(e) => onChange({ ...server, security: e.target.value as Security })}>
          <option value="tls">{t("setup.tls")}</option>
          <option value="start_tls">{t("setup.starttls")}</option>
          <option value="plain">{t("setup.plain")}</option>
        </Select>
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
  const [pop3, setPop3] = useState<Pop3Transport | null>(a.transport.kind === "pop3" ? a.transport : null);
  const [password, setPassword] = useState("");
  const [state, setState] = useState<{ busy: boolean; message: string | null; ok: boolean }>({ busy: false, message: null, ok: false });
  const [confirm, setConfirm] = useState(false);

  useEffect(() => {
    setName(a.name);
    setSender(a.sender_name);
    setSignature(a.signature);
    setTransport(a.transport.kind === "imap" ? a.transport : null);
    setPop3(a.transport.kind === "pop3" ? a.transport : null);
  }, [a.id]);

  // What was saved last: leaving a field without a change saves nothing.
  const saved = useRef({ name: a.name, sender: a.sender_name, signature: a.signature });
  useEffect(() => {
    saved.current = { name: a.name, sender: a.sender_name, signature: a.signature };
  }, [a.id]);
  const profileChanged = name !== saved.current.name || sender !== saved.current.sender || signature !== saved.current.signature;
  const saveProfile = async () => {
    if (name === saved.current.name && sender === saved.current.sender && signature === saved.current.signature) return;
    saved.current = { name, sender, signature };
    await api.updateAccount(a.id, name, sender, signature);
    onChanged();
  };
  useSaveOnLeave(profileChanged, () => void saveProfile());

  const saveServers = async () => {
    const current: ImapTransport | Pop3Transport | null = pop3 ?? transport;
    if (!current) return;
    setState({ busy: true, message: null, ok: false });
    try {
      if (password) {
        const r = await api.check(current, password);
        const p = r.imap ?? r.smtp;
        if (p || r.error) {
          const e = p ? explain(p) : null;
          setState({ busy: false, message: e ? `${t(e.title, e.vars)} – ${t(e.hint, e.vars)}` : (r.error ?? ""), ok: false });
          return;
        }
      }
      await api.updateServers(a.id, current, password || null);
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
            <input className="input" value={transport.username} spellCheck={false} autoCapitalize="off" autoCorrect="off" onChange={(e) => setTransport({ ...transport, username: e.target.value })} />
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
      {pop3 && (
        <div className="set-card">
          <div className="set-card-title">{t("settings.servers")}</div>
          <label className="stack-field">
            <span className="field-label">{t("setup.username")}</span>
            <input className="input" value={pop3.username} spellCheck={false} autoCapitalize="off" autoCorrect="off" onChange={(e) => setPop3({ ...pop3, username: e.target.value })} />
          </label>
          <ServerEdit label={t("setup.incomingPop3")} server={pop3.pop3} onChange={(s) => setPop3({ ...pop3, pop3: s })} />
          <ServerEdit label={t("setup.outgoing")} server={pop3.smtp} onChange={(smtp) => setPop3({ ...pop3, smtp })} />
          <Row label={t("setup.leaveOnServer")} description={t("setup.leaveOnServerHint")}>
            <Switch label={t("setup.leaveOnServer")} checked={pop3.leave_on_server} onChange={(leave_on_server) => setPop3({ ...pop3, leave_on_server })} />
          </Row>
          {pop3.leave_on_server && (
            <Row label={t("setup.deleteAfter")} description={t("setup.deleteAfterHint")}>
              <Select
                className="select"
                aria-label={t("setup.deleteAfter")}
                value={pop3.delete_after_days ?? 0}
                onChange={(e) => setPop3({ ...pop3, delete_after_days: Number(e.target.value) || null })}
              >
                <option value={0}>{t("setup.keepForever")}</option>
                {[7, 14, 30, 90].map((n) => (
                  <option key={n} value={n}>
                    {t("setup.afterDays", { n })}
                  </option>
                ))}
              </Select>
            </Row>
          )}
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
      <SyncDetails accountId={a.id} />
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
  ["ai", "net.routeAi"],
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
  useSaveOnLeave(JSON.stringify(draft) !== JSON.stringify(net), () => commit(draft));
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
                <input className="input" value={p.http_proxy} placeholder={t("net.proxyPlaceholder")} spellCheck={false} autoCapitalize="off" autoCorrect="off" onChange={(e) => setProfile(i, { http_proxy: e.target.value }, false)} onBlur={() => commit(draft)} />
              </label>
              <label className="stack-field">
                <span className="field-label">{t("net.socksProxy")}</span>
                <input className="input" value={p.socks_proxy} placeholder={t("net.proxyPlaceholder")} spellCheck={false} autoCapitalize="off" autoCorrect="off" onChange={(e) => setProfile(i, { socks_proxy: e.target.value }, false)} onBlur={() => commit(draft)} />
              </label>
              <label className="stack-field">
                <span className="field-label">{t("net.noProxy")}</span>
                <input className="input" value={p.no_proxy} spellCheck={false} autoCapitalize="off" autoCorrect="off" onChange={(e) => setProfile(i, { no_proxy: e.target.value }, false)} onBlur={() => commit(draft)} />
                <span className="field-hint">{t("net.noProxyHint")}</span>
              </label>
              <label className="stack-field">
                <span className="field-label">{t("net.user")}</span>
                <input className="input" value={p.proxy_user} spellCheck={false} autoCapitalize="off" autoCorrect="off" onChange={(e) => setProfile(i, { proxy_user: e.target.value }, false)} onBlur={() => commit(draft)} />
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
            <Select
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
            </Select>
          </Row>
        ))}
      </div>
    </div>
  );
}

type Props = {
  /** Settings → Posteingang → Abos: the subscriptions in the list pane. */
  onSubscriptions: () => void;
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

export function SettingsView({ settings, accounts, info, section, onSection, onChange, onAccountsChanged, onClose, onSubscriptions, touch, phone }: Props) {
  const t = useT();
  const mobile = info?.platform === "android" || info?.platform === "ios";
  const mac = usesCommand(info?.platform);
  const keyNames = { ctrl: t("keys.ctrl"), shift: t("keys.shift"), enter: t("keys.enter"), del: t("keys.del") };
  const [accountId, setAccountId] = useState<number | null>(accounts[0]?.id ?? null);
  const [adding, setAdding] = useState(false);
  const [senders, setSenders] = useState<string[]>([]);
  const [blocked, setBlocked] = useState<string[]>([]);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (section === "privacy") void api.allowedSenders().then(setSenders);
    if (section === "inbox") void api.blockedSenders().then(setBlocked);
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
          {SECTIONS.filter(([id]) => !(phone && id === "shortcuts")).map(([id, label, Icon]) => (
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
                  <Select className="select" value={settings.language} onChange={(e) => onChange({ language: e.target.value as LanguageChoice })} aria-label={t("lang.label")}>
                    <option value="system">{t("theme.system")}</option>
                    <option value="de">{t("lang.de")}</option>
                    <option value="en">{t("lang.en")}</option>
                  </Select>
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
                <Row label={t("settings.startupAnimation")} description={t("settings.startupAnimationHint")}>
                  <Switch label={t("settings.startupAnimation")} checked={settings.startup_animation} onChange={(startup_animation) => onChange({ startup_animation })} />
                </Row>
              </div>
            )}
            {section === "reading" && (
              <div className="set-card">
                <Row label={t("settings.markRead")} description={t("settings.markReadHint")}>
                  <Select
                    className="select"
                    aria-label={t("settings.markRead")}
                    value={settings.mark_read_after_secs == null ? "never" : String(settings.mark_read_after_secs)}
                    onChange={(e) => onChange({ mark_read_after_secs: e.target.value === "never" ? null : Number(e.target.value) })}
                  >
                    <option value="0">{t("settings.markReadNow")}</option>
                    <option value="2">{t("settings.markReadAfter", { n: 2 })}</option>
                    <option value="never">{t("settings.markReadNever")}</option>
                  </Select>
                </Row>
                <Row label={t("settings.prefetch")} description={t("settings.prefetchHint")}>
                  <Select className="select" aria-label={t("settings.prefetch")} value={settings.prefetch} onChange={(e) => onChange({ prefetch: e.target.value as Prefetch })}>
                    <option value="none">{t("settings.prefetchNone")}</option>
                    <option value="recent">{t("settings.prefetchRecent")}</option>
                    <option value="month">{t("settings.prefetchMonth")}</option>
                  </Select>
                </Row>
              </div>
            )}
            {section === "reading" && touch && (
              <div className="set-card" aria-label={t("settings.gestures")} role="group">
                {(["swipe_left", "swipe_right"] as const).map((key) => (
                  <Row key={key} label={t(key === "swipe_left" ? "settings.swipeLeft" : "settings.swipeRight")} description={t("settings.swipeHint")}>
                    <Select
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
                    </Select>
                  </Row>
                ))}
              </div>
            )}
            {section === "inbox" && (
              <>
                <div className="set-card">
                  <Row label={t("settings.screener")} description={t("settings.screenerHint")}>
                    <Switch label={t("settings.screener")} checked={settings.screener} onChange={(screener) => onChange({ screener })} />
                  </Row>
                  <Row label={t("settings.focus")} description={t("settings.focusHint")}>
                    <Switch label={t("settings.focus")} checked={settings.focus} onChange={(focus) => onChange({ focus })} />
                  </Row>
                  <Row label={t("settings.unified")} description={t("settings.unifiedHint")}>
                    <Switch label={t("settings.unified")} checked={settings.unified_inbox} onChange={(unified_inbox) => onChange({ unified_inbox })} />
                  </Row>
                  <Row label={t("settings.bundles")} description={t("settings.bundlesHint")}>
                    <Switch label={t("settings.bundles")} checked={settings.bundles} onChange={(bundles) => onChange({ bundles })} />
                  </Row>
                </div>
                <div className="set-card">
                  <Row label={t("settings.subscriptions")} description={t("settings.subscriptionsHint")}>
                    <button
                      type="button"
                      className="btn"
                      onClick={() => {
                        onSubscriptions();
                        onClose();
                      }}
                    >
                      {t("toast.show")}
                    </button>
                  </Row>
                  <Row label={t("settings.trackers")} description={t("settings.trackersHint")}>
                    <span className="set-badge">{t("settings.alwaysOn")}</span>
                  </Row>
                  <div className="set-row set-row-stack">
                    <div className="set-text">
                      <div className="set-label">{t("settings.blockedSenders")}</div>
                      <div className="set-desc">{t("settings.blockedSendersHint")}</div>
                    </div>
                    {blocked.length === 0 ? (
                      <p className="muted set-empty">{t("settings.noBlockedSenders")}</p>
                    ) : (
                      <ul className="sender-list">
                        {blocked.map((s) => (
                          <li key={s}>
                            <span>{s}</span>
                            <button type="button" className="icon-btn icon-btn-sm" aria-label={t("settings.letIn", { email: s })} title={t("settings.letIn", { email: s })} onClick={() => void api.screenerForget(s).then(() => setBlocked(blocked.filter((x) => x !== s)))}>
                              <X size={14} strokeWidth={1.75} aria-hidden />
                            </button>
                          </li>
                        ))}
                      </ul>
                    )}
                  </div>
                </div>
              </>
            )}
            {section === "network" && <NetworkSection net={settings.network} onChange={(network) => onChange({ network })} />}
            {section === "ai" && <AiSection settings={settings} accounts={accounts} onChange={onChange} />}
            {section === "rules" && <RulesSection accounts={accounts} />}
            {section === "templates" && <TemplatesSection accounts={accounts} />}
            {section === "contacts" && <ContactsSection settings={settings} accounts={accounts} onChange={onChange} />}
            {section === "calendar" && <CalendarSection settings={settings} onChange={onChange} />}
            {section === "encryption" && <CryptoSection settings={settings} accounts={accounts} onChange={onChange} />}
            {section === "security" && <SecuritySection settings={settings} onChange={onChange} />}
            {section === "writing" && (
              <div className="set-card">
                <Row label={t("settings.richText")} description={t("settings.richTextHint")}>
                  <Switch label={t("settings.richText")} checked={settings.compose_rich_text} onChange={(compose_rich_text) => onChange({ compose_rich_text })} />
                </Row>
                <Row label={t("settings.undoSend")} description={t("settings.undoSendHint")}>
                  <Select className="select" aria-label={t("settings.undoSend")} value={String(settings.undo_send_secs)} onChange={(e) => onChange({ undo_send_secs: Number(e.target.value) })}>
                    <option value="0">{t("settings.undoSendOff")}</option>
                    {[5, 10, 20, 30].map((n) => (
                      <option key={n} value={n}>
                        {t("settings.seconds", { n })}
                      </option>
                    ))}
                  </Select>
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
                  <Row label={t("settings.aiPrivacy")} description={settings.ai.providers.length ? t("settings.aiPrivacyOn") : t("settings.aiPrivacyOff")}>
                    <button type="button" className="btn btn-sm" onClick={() => onSection("ai")}>
                      {t("settings.aiOpen")}
                    </button>
                  </Row>
                </div>
              </>
            )}
            {section === "shortcuts" && (
              <div className="set-card">
                <table className="keys">
                  <tbody>
                    {SHORTCUTS.map(({ keys, label }) => (
                      <tr key={label}>
                        <td>
                          {keys.map((combo, i) => (
                            <span key={combo.join("+")}>
                              {i > 0 && " / "}
                              <kbd>{combo.map((k) => keyLabel(k, mac, keyNames)).join(mac ? "" : "+")}</kbd>
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
              <>
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
                <UpdateSettings mode={settings.updates} onMode={(updates) => onChange({ updates })} platform={info?.platform ?? "browser"} version={info?.version ?? ""} />
                <DefaultMailCard />
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
