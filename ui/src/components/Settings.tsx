import { Bell, Info, Keyboard, Palette, PenLine, Shield, Trash2, UserRound, X, Plus } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { useT, type TKey } from "../lib/i18n";
import { api } from "../lib/api";
import type { AccountInfo, AppInfo, ImapTransport, LanguageChoice, Security, Server, Settings as SettingsT, ThemeChoice } from "../lib/types";
import { explain } from "../lib/problems";
import { ConfirmDialog, Segmented, Spinner, Switch, trapFocus } from "./ui";
import { Setup } from "./Setup";
import { useRef } from "react";

export type SettingsSection = "accounts" | "appearance" | "reading" | "writing" | "notifications" | "privacy" | "shortcuts" | "about";

const SECTIONS: [SettingsSection, TKey, typeof Bell][] = [
  ["accounts", "settings.accounts", UserRound],
  ["appearance", "settings.appearance", Palette],
  ["reading", "settings.reading", Info],
  ["writing", "settings.writing", PenLine],
  ["notifications", "settings.notifications", Bell],
  ["privacy", "settings.privacy", Shield],
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
];

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
      {transport && (
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

type Props = {
  settings: SettingsT;
  accounts: AccountInfo[];
  info: AppInfo | null;
  section: SettingsSection;
  onSection: (s: SettingsSection) => void;
  onChange: (patch: Partial<SettingsT>) => void;
  onAccountsChanged: () => void;
  onClose: () => void;
};

export function SettingsView({ settings, accounts, info, section, onSection, onChange, onAccountsChanged, onClose }: Props) {
  const t = useT();
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
          <div className="settings-nav-title">{t("settings.title")}</div>
          {SECTIONS.map(([id, label, Icon]) => (
            <button key={id} type="button" className="settings-nav-item" aria-current={section === id ? "page" : undefined} onClick={() => onSection(id)}>
              <Icon size={15} strokeWidth={1.75} aria-hidden />
              <span>{t(label)}</span>
            </button>
          ))}
        </nav>
        <div className="settings-main">
          <header className="settings-head">
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
              </div>
            )}
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
                  <Row label={t("settings.passwords")} description={store?.kind === "file" ? t("settings.passwordsFile", { reason: store.reason }) : t("settings.passwordsSystem")}>
                    <span className="set-badge">{store?.kind === "file" ? t("settings.storeFile") : t("settings.storeSystem")}</span>
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
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
