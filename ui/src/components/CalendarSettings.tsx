// Settings → Kalender (the calendars, where they come from, their colors, the default one, the
// work week) and Settings → Verschlüsselung (OpenPGP keys, S/MIME certificates, Autocrypt,
// signing by default). The secrets (CalDAV passwords, ICS addresses, secret keys, certificate
// files) go to the credential store in the backend; the UI never keeps them.

import { useEffect, useState, type CSSProperties, type ReactNode } from "react";
import { Download, Eye, EyeOff, KeyRound, Plus, RefreshCw, Search, ShieldCheck, Trash2, Upload } from "lucide-react";
import { api, pickFiles, pickSavePath } from "../lib/api";
import { currentLang, useT, type TKey } from "../lib/i18n";
import { listDate, now } from "../lib/format";
import { showToast } from "../lib/toasts";
import { CAL_PALETTE } from "../lib/calendar";
import type { AccountInfo, CalendarInfo, CalendarOverview, Settings, StoredKey } from "../lib/types";
import { ConfirmDialog, Dialog, Segmented, Spinner, Switch } from "./ui";

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

const WEEKDAYS = [1, 2, 3, 4, 5, 6, 7];

/** Where a key came from. */
const SOURCES: Record<string, TKey> = {
  generated: "crypto.source.generated",
  imported: "crypto.source.imported",
  autocrypt: "crypto.source.autocrypt",
  wkd: "crypto.source.wkd",
  mail: "crypto.source.mail",
};

/** The day's short name (Monday = 1). */
const dayName = (n: number) => new Date(2024, 0, n).toLocaleDateString(currentLang() === "de" ? "de-DE" : "en-GB", { weekday: "short" }).replace(".", "");

// ---------------------------------------------------------------- calendars

function CalDavDialog({ presets, onAdded, onClose }: { presets: [string, string][]; onAdded: () => void; onClose: () => void }) {
  const t = useT();
  const [preset, setPreset] = useState(0);
  const [url, setUrl] = useState(presets[0]?.[1] ?? "https://");
  const [host, setHost] = useState("");
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [state, setState] = useState<{ busy: boolean; error: string | null }>({ busy: false, error: null });
  const withHost = url.includes("{host}");
  const address = withHost ? url.replace("{host}", host.trim() || "{host}") : url;
  return (
    <Dialog
      title={t("calset.addCaldav")}
      description={t("calset.addCaldavHint")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-quiet" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={state.busy || !user || !password || address.includes("{host}")}
            onClick={() => {
              setState({ busy: true, error: null });
              void api.caldavAdd(preset < presets.length ? presets[preset][0] : "", address, user, password).then(
                () => {
                  onAdded();
                  onClose();
                },
                (e) => setState({ busy: false, error: String(e) }),
              );
            }}
          >
            {state.busy && <Spinner />}
            <span>{t("calset.connect")}</span>
          </button>
        </>
      }
    >
      <label className="stack-field">
        <span className="field-label">{t("calset.service")}</span>
        <select
          className="select"
          value={preset}
          onChange={(e) => {
            const i = Number(e.target.value);
            setPreset(i);
            setUrl(i < presets.length ? presets[i][1] : "https://");
          }}
        >
          {presets.map(([name], i) => (
            <option key={name} value={i}>
              {name}
            </option>
          ))}
          <option value={presets.length}>{t("calset.otherServer")}</option>
        </select>
      </label>
      {withHost ? (
        <label className="stack-field">
          <span className="field-label">{t("calset.host")}</span>
          <input className="input" value={host} spellCheck={false} placeholder="cloud.example.org" onChange={(e) => setHost(e.target.value.trim())} />
        </label>
      ) : (
        <label className="stack-field">
          <span className="field-label">{t("calset.address")}</span>
          <input className="input" value={url} spellCheck={false} onChange={(e) => setUrl(e.target.value.trim())} />
        </label>
      )}
      <label className="stack-field">
        <span className="field-label">{t("setup.username")}</span>
        <input className="input" value={user} spellCheck={false} autoComplete="off" onChange={(e) => setUser(e.target.value)} />
      </label>
      <label className="stack-field">
        <span className="field-label">{t("calset.appPassword")}</span>
        <input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
        <span className="field-hint">{t("calset.appPasswordHint")}</span>
      </label>
      {state.error && (
        <p className="field-error" role="alert">
          {state.error}
        </p>
      )}
    </Dialog>
  );
}

function IcsDialog({ onAdded, onClose }: { onAdded: () => void; onClose: () => void }) {
  const t = useT();
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [state, setState] = useState<{ busy: boolean; error: string | null }>({ busy: false, error: null });
  return (
    <Dialog
      title={t("calset.addIcs")}
      description={t("calset.addIcsHint")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-quiet" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={state.busy || !url.trim()}
            onClick={() => {
              setState({ busy: true, error: null });
              void api.icsSubscribe(name, url.trim()).then(
                () => {
                  onAdded();
                  onClose();
                },
                (e) => setState({ busy: false, error: String(e) }),
              );
            }}
          >
            {state.busy && <Spinner />}
            <span>{t("calset.subscribe")}</span>
          </button>
        </>
      }
    >
      <label className="stack-field">
        <span className="field-label">{t("calset.icsAddress")}</span>
        <input className="input" value={url} spellCheck={false} placeholder="webcal://…" onChange={(e) => setUrl(e.target.value)} />
        <span className="field-hint">{t("calset.icsPrivate")}</span>
      </label>
      <label className="stack-field">
        <span className="field-label">{t("calset.name")}</span>
        <input className="input" value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      {state.error && (
        <p className="field-error" role="alert">
          {state.error}
        </p>
      )}
    </Dialog>
  );
}

/** A calendar's row: its color (the palette), shown or hidden. */
function CalendarRow({ c, onChanged }: { c: CalendarInfo; onChanged: (c: CalendarInfo) => void }) {
  const t = useT();
  const set = (patch: { color?: string; hidden?: boolean }) =>
    void api
      .calendarSet(c.id, patch)
      .then(onChanged)
      .catch((e) => showToast({ title: t("common.notSaved"), detail: String(e), tone: "danger" }));
  return (
    <li className="calset-cal" style={{ "--ev": c.color } as CSSProperties}>
      <span className="calv-dot" aria-hidden />
      <span className="calset-cal-name">{c.name}</span>
      {!c.writable && <span className="muted calset-cal-note">{t("calset.readOnly")}</span>}
      <span className="calset-swatches" role="radiogroup" aria-label={t("calset.colorOf", { name: c.name })}>
        {CAL_PALETTE.map((p) => (
          <button
            key={p}
            type="button"
            role="radio"
            aria-checked={p.toLowerCase() === c.color.toLowerCase()}
            className="calset-swatch"
            style={{ "--ev": p } as CSSProperties}
            aria-label={p}
            onClick={() => set({ color: p })}
          />
        ))}
      </span>
      <button type="button" className="icon-btn icon-btn-sm" aria-pressed={!c.hidden} aria-label={c.hidden ? t("calv.show", { name: c.name }) : t("calv.hide", { name: c.name })} title={c.hidden ? t("calv.show", { name: c.name }) : t("calv.hide", { name: c.name })} onClick={() => set({ hidden: !c.hidden })}>
        {c.hidden ? <EyeOff size={14} strokeWidth={1.75} aria-hidden /> : <Eye size={14} strokeWidth={1.75} aria-hidden />}
      </button>
    </li>
  );
}

export function CalendarSection({ settings, onChange }: { settings: Settings; onChange: (patch: Partial<Settings>) => void }) {
  const t = useT();
  const lang = currentLang();
  const [o, setO] = useState<CalendarOverview | null>(null);
  const [adding, setAdding] = useState<"caldav" | "ics" | null>(null);
  const [busy, setBusy] = useState<number | null>(null);
  const [removing, setRemoving] = useState<{ id: number; name: string } | null>(null);
  const load = () => void api.calendarOverview().then(setO);
  useEffect(load, []);
  const cal = settings.calendar;
  const setCal = (patch: Partial<Settings["calendar"]>) => onChange({ calendar: { ...cal, ...patch } });
  const writable = o?.calendars.filter((c) => c.writable && c.kind !== "ics") ?? [];
  const sourceLine = (id: number) => {
    const s = o?.sources.find((x) => x.id === id);
    if (!s) return "";
    if (s.error) return s.error;
    const when = s.last_sync ? listDate(s.last_sync, now(), lang) : "–";
    return s.pending ? t("calset.sourceInfoPending", { when, n: s.pending }) : t("calset.sourceInfo", { when });
  };
  return (
    <div className="set-section">
      <p className="muted set-intro">{t("calset.intro")}</p>
      <div className="set-card">
        <div className="set-card-title">{t("calset.calendars")}</div>
        {o === null ? (
          <Spinner />
        ) : o.sources.length === 0 ? (
          <p className="muted set-empty">{t("calset.none")}</p>
        ) : (
          o.sources.map((s) => (
            <div key={s.id} className="calset-source">
              <div className="rule-item">
                <span className="rule-text">
                  <span className="rule-name">{s.kind === "local" ? t("calset.local") : s.name}</span>
                  <span className={s.error ? "field-error rule-summary" : "muted rule-summary"}>{s.kind === "local" ? t("calset.localHint") : `${t(`calset.kind.${s.kind}` as const)} · ${sourceLine(s.id)}`}</span>
                </span>
                {s.kind !== "local" && (
                  <>
                    <button
                      type="button"
                      className="icon-btn icon-btn-sm"
                      disabled={busy === s.id}
                      aria-label={t("calset.syncNamed", { name: s.name })}
                      onClick={() => {
                        setBusy(s.id);
                        void api
                          .calendarSync()
                          .catch((e) => showToast({ title: t("calv.syncFailed"), detail: String(e), tone: "danger" }))
                          .finally(() => {
                            setBusy(null);
                            load();
                          });
                      }}
                    >
                      {busy === s.id ? <Spinner /> : <RefreshCw size={14} strokeWidth={1.75} aria-hidden />}
                    </button>
                    <button type="button" className="icon-btn icon-btn-sm" aria-label={t("calset.removeNamed", { name: s.name })} onClick={() => setRemoving({ id: s.id, name: s.name })}>
                      <Trash2 size={14} strokeWidth={1.75} aria-hidden />
                    </button>
                  </>
                )}
              </div>
              <ul className="calset-cals">
                {o.calendars
                  .filter((c) => c.source_id === s.id)
                  .map((c) => (
                    <CalendarRow key={c.id} c={c} onChanged={(n) => setO((x) => (x ? { ...x, calendars: x.calendars.map((y) => (y.id === n.id ? n : y)) } : x))} />
                  ))}
              </ul>
            </div>
          ))
        )}
        <div className="set-actions set-actions-left">
          <button type="button" className="btn btn-sm" onClick={() => setAdding("caldav")}>
            <Plus size={14} strokeWidth={1.75} aria-hidden />
            <span>{t("calset.addCaldav")}</span>
          </button>
          <button type="button" className="btn btn-sm" onClick={() => setAdding("ics")}>
            <Plus size={14} strokeWidth={1.75} aria-hidden />
            <span>{t("calset.addIcs")}</span>
          </button>
        </div>
      </div>
      {(o?.accounts.length ?? 0) > 0 && (
        <div className="set-card">
          {o!.accounts.map((a) => (
            <Row key={a.account_id} label={t(a.kind === "graph" ? "calset.microsoft" : "calset.jmap", { email: a.email })} description={t(a.kind === "graph" ? "calset.microsoftHint" : "calset.jmapHint")}>
              <Switch
                label={t(a.kind === "graph" ? "calset.microsoft" : "calset.jmap", { email: a.email })}
                checked={a.on}
                onChange={(on) => {
                  setBusy(a.account_id);
                  void api
                    .calendarAccount(a.account_id, on)
                    .catch((e) => showToast({ title: t("calv.syncFailed"), detail: String(e), tone: "danger" }))
                    .finally(() => {
                      setBusy(null);
                      load();
                    });
                }}
              />
            </Row>
          ))}
        </div>
      )}
      <div className="set-card">
        <Row label={t("calset.default")} description={t("calset.defaultHint")}>
          <select className="select" value={cal.default_calendar ?? o?.default ?? ""} onChange={(e) => setCal({ default_calendar: Number(e.target.value) })} aria-label={t("calset.default")}>
            {writable.length === 0 && <option value="">{t("calset.local")}</option>}
            {writable.map((c) => (
              <option key={c.id} value={c.id}>
                {c.source_name && c.source_name !== c.name ? `${c.name} · ${c.source_name}` : c.name}
              </option>
            ))}
          </select>
        </Row>
        <Row label={t("calset.length")} description={t("calset.lengthHint")}>
          <Segmented
            label={t("calset.length")}
            value={String(cal.entry_minutes)}
            onChange={(v) => setCal({ entry_minutes: Number(v) })}
            options={["15", "30", "45", "60"].map((m) => [m, t("calset.minutes", { n: Number(m) })] as [string, string])}
          />
        </Row>
        <Row label={t("calset.workdays")} description={t("calset.workdaysHint")}>
          <span className="calset-days" role="group" aria-label={t("calset.workdays")}>
            {WEEKDAYS.map((d) => {
              const on = cal.workdays.includes(d);
              return (
                <button key={d} type="button" className={`calset-day${on ? " on" : ""}`} aria-pressed={on} onClick={() => setCal({ workdays: on ? cal.workdays.filter((x) => x !== d) : [...cal.workdays, d].sort() })}>
                  {dayName(d)}
                </button>
              );
            })}
          </span>
        </Row>
      </div>
      {adding === "caldav" && <CalDavDialog presets={o?.presets ?? []} onAdded={load} onClose={() => setAdding(null)} />}
      {adding === "ics" && <IcsDialog onAdded={load} onClose={() => setAdding(null)} />}
      {removing && (
        <ConfirmDialog
          title={t("calset.removeTitle", { name: removing.name })}
          text={t("calset.removeText")}
          confirm={t("calset.remove")}
          danger
          onConfirm={() => void api.calendarRemoveSource(removing.id).then(load)}
          onClose={() => setRemoving(null)}
        />
      )}
    </div>
  );
}

// ---------------------------------------------------------------- encryption

function ImportKeyDialog({ onDone, onClose }: { onDone: () => void; onClose: () => void }) {
  const t = useT();
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  /** A protected secret key: its passphrase, checked on import. */
  const [passphrase, setPassphrase] = useState<string | null>(null);
  const [remember, setRemember] = useState(false);
  const run = async () => {
    setError(null);
    try {
      if (passphrase == null && (await api.pgpNeedsPassphrase(text))) {
        setPassphrase("");
        return;
      }
      const keys = await api.pgpImport(text, passphrase, remember);
      showToast({ title: t("crypto.imported", { n: keys.length }), tone: "success", duration: 3000 });
      onDone();
      onClose();
    } catch (e) {
      setError(String(e));
    }
  };
  return (
    <Dialog
      title={t("crypto.importPgp")}
      description={t("crypto.importPgpHint")}
      onClose={onClose}
      wide
      footer={
        <>
          <button type="button" className="btn btn-quiet" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button type="button" className="btn btn-primary" disabled={!text.trim() || passphrase === ""} onClick={() => void run()}>
            {t("crypto.import")}
          </button>
        </>
      }
    >
      <textarea
        className="input crypto-armor"
        value={text}
        spellCheck={false}
        placeholder="-----BEGIN PGP PUBLIC KEY BLOCK-----"
        onChange={(e) => {
          setText(e.target.value);
          setPassphrase(null);
        }}
        aria-label={t("crypto.importPgp")}
      />
      {passphrase != null && (
        <>
          <label className="stack-field">
            <span className="field-label">{t("crypto.passphraseNeeded")}</span>
            <input className="input" type="password" autoFocus aria-label={t("protect.passphrase")} value={passphrase} onChange={(e) => setPassphrase(e.target.value)} onKeyDown={(e) => e.key === "Enter" && passphrase && void run()} />
          </label>
          <div className="set-control-row passphrase-remember">
            <Switch label={t("protect.rememberPassphrase")} checked={remember} onChange={setRemember} />
            <span>{t("protect.rememberPassphrase")}</span>
          </div>
          <p className="muted">{remember ? t("protect.rememberHint") : t("protect.forNowHint")}</p>
        </>
      )}
      {error && (
        <p className="field-error" role="alert">
          {error}
        </p>
      )}
    </Dialog>
  );
}

function SmimeDialog({ onDone, onClose }: { onDone: () => void; onClose: () => void }) {
  const t = useT();
  const [path, setPath] = useState<string | null>(null);
  const [password, setPassword] = useState("");
  const [state, setState] = useState<{ busy: boolean; error: string | null }>({ busy: false, error: null });
  return (
    <Dialog
      title={t("crypto.importSmime")}
      description={t("crypto.importSmimeHint")}
      onClose={onClose}
      footer={
        <>
          <button type="button" className="btn btn-quiet" onClick={onClose}>
            {t("common.cancel")}
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={!path || state.busy}
            onClick={() => {
              setState({ busy: true, error: null });
              void api.smimeImport(path!, password).then(
                () => {
                  onDone();
                  onClose();
                },
                (e) => setState({ busy: false, error: String(e) }),
              );
            }}
          >
            {state.busy && <Spinner />}
            <span>{t("crypto.import")}</span>
          </button>
        </>
      }
    >
      <div className="stack-field">
        <span className="field-label">{t("crypto.p12File")}</span>
        <span className="crypto-file">
          <button type="button" className="btn btn-sm" onClick={() => void pickFiles().then((p) => p[0] && setPath(p[0]))}>
            <Upload size={14} strokeWidth={1.75} aria-hidden />
            <span>{t("crypto.chooseFile")}</span>
          </button>
          <span className="muted ellipsis">{path ? path.split(/[\\/]/).pop() : t("crypto.noFile")}</span>
        </span>
      </div>
      <label className="stack-field">
        <span className="field-label">{t("crypto.p12Password")}</span>
        <input className="input" type="password" value={password} onChange={(e) => setPassword(e.target.value)} />
      </label>
      {state.error && (
        <p className="field-error" role="alert">
          {state.error}
        </p>
      )}
    </Dialog>
  );
}

export function CryptoSection({ settings, accounts, onChange }: { settings: Settings; accounts: AccountInfo[]; onChange: (patch: Partial<Settings>) => void }) {
  const t = useT();
  const lang = currentLang();
  const [keys, setKeys] = useState<StoredKey[] | null>(null);
  const [dialog, setDialog] = useState<"pgp" | "smime" | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [lookup, setLookup] = useState("");
  const load = () => void api.cryptoKeys().then(setKeys);
  useEffect(load, []);
  const own = keys?.filter((k) => k.own) ?? [];
  const others = keys?.filter((k) => !k.own) ?? [];
  const generate = (accountId: number) => {
    setBusy(`gen-${accountId}`);
    void api
      .pgpGenerate(accountId)
      .then(() => showToast({ title: t("crypto.generated"), tone: "success", duration: 3000 }))
      .catch((e) => showToast({ title: t("crypto.failed"), detail: String(e), tone: "danger" }))
      .finally(() => {
        setBusy(null);
        load();
      });
  };
  const exportKey = async (k: StoredKey) => {
    const path = await pickSavePath(`${k.email || "key"}.asc`);
    if (!path) return;
    try {
      const armored = await api.pgpExport(k.fingerprint);
      await api.saveMarkdown(path, armored);
      showToast({ title: t("crypto.exported"), duration: 2500 });
    } catch (e) {
      showToast({ title: t("crypto.failed"), detail: String(e), tone: "danger" });
    }
  };
  const keyRow = (k: StoredKey) => (
    <li key={`${k.kind}-${k.fingerprint}`} className="rule-item">
      <span className="rule-text">
        <span className="rule-name">
          {k.name ? `${k.name} <${k.email}>` : k.email}
          <span className="crypto-kind">{k.kind === "pgp" ? "OpenPGP" : "S/MIME"}</span>
        </span>
        <span className="muted rule-summary crypto-fp">
          {k.fingerprint.replace(/(.{4})/g, "$1 ").trim().slice(0, 49)}
          {k.expires ? ` · ${t("crypto.until", { when: listDate(k.expires, now(), lang) })}` : ""}
          {` · ${t(SOURCES[k.source] ?? "crypto.source.imported")}`}
        </span>
      </span>
      {k.kind === "pgp" && (
        <button type="button" className="icon-btn icon-btn-sm" aria-label={t("crypto.exportNamed", { name: k.email })} title={t("crypto.export")} onClick={() => void exportKey(k)}>
          <Download size={14} strokeWidth={1.75} aria-hidden />
        </button>
      )}
      <button type="button" className="icon-btn icon-btn-sm" aria-label={t("crypto.removeNamed", { name: k.email })} onClick={() => void api.cryptoRemove(k.kind, k.fingerprint).then(load)}>
        <Trash2 size={14} strokeWidth={1.75} aria-hidden />
      </button>
    </li>
  );
  return (
    <div className="set-section">
      <p className="muted set-intro">{t("crypto.intro")}</p>
      <div className="set-card">
        <div className="set-card-title">{t("crypto.own")}</div>
        {keys === null ? <Spinner /> : own.length === 0 ? <p className="muted set-empty">{t("crypto.noOwn")}</p> : <ul className="rule-list">{own.map(keyRow)}</ul>}
        <div className="set-actions set-actions-left">
          {accounts
            .filter((a) => !own.some((k) => k.kind === "pgp" && k.email.toLowerCase() === a.email.toLowerCase()))
            .map((a) => (
              <button key={a.id} type="button" className="btn btn-sm" disabled={busy === `gen-${a.id}`} onClick={() => generate(a.id)}>
                {busy === `gen-${a.id}` ? <Spinner /> : <KeyRound size={14} strokeWidth={1.75} aria-hidden />}
                <span>{t("crypto.generateFor", { email: a.email })}</span>
              </button>
            ))}
          <button type="button" className="btn btn-sm" onClick={() => setDialog("pgp")}>
            <Upload size={14} strokeWidth={1.75} aria-hidden />
            <span>{t("crypto.importPgp")}</span>
          </button>
          <button type="button" className="btn btn-sm" onClick={() => setDialog("smime")}>
            <ShieldCheck size={14} strokeWidth={1.75} aria-hidden />
            <span>{t("crypto.importSmime")}</span>
          </button>
        </div>
      </div>
      <div className="set-card">
        <div className="set-card-title">{t("crypto.others")}</div>
        {keys !== null && (others.length === 0 ? <p className="muted set-empty">{t("crypto.noOthers")}</p> : <ul className="rule-list">{others.map(keyRow)}</ul>)}
        <form
          className="crypto-lookup"
          onSubmit={(e) => {
            e.preventDefault();
            if (!lookup.trim()) return;
            setBusy("lookup");
            void api
              .pgpLookup(lookup.trim())
              .then((k) => showToast({ title: k ? t("crypto.found", { email: k.email }) : t("crypto.notFound", { email: lookup.trim() }), duration: 3500, tone: k ? "success" : "info" }))
              .catch((err) => showToast({ title: t("crypto.failed"), detail: String(err), tone: "danger" }))
              .finally(() => {
                setBusy(null);
                load();
              });
          }}
        >
          <input className="input" type="email" value={lookup} placeholder={t("crypto.lookupPlaceholder")} aria-label={t("crypto.lookup")} onChange={(e) => setLookup(e.target.value)} />
          <button type="submit" className="btn btn-sm" disabled={busy === "lookup" || !lookup.trim()}>
            {busy === "lookup" ? <Spinner /> : <Search size={14} strokeWidth={1.75} aria-hidden />}
            <span>{t("crypto.lookup")}</span>
          </button>
        </form>
        <p className="muted set-desc">{t("crypto.lookupHint")}</p>
      </div>
      <div className="set-card">
        <Row label={t("crypto.autocrypt")} description={t("crypto.autocryptHint")}>
          <Switch label={t("crypto.autocrypt")} checked={settings.autocrypt} onChange={(autocrypt) => onChange({ autocrypt })} />
        </Row>
        <Row label={t("crypto.signDefault")} description={t("crypto.signDefaultHint")}>
          <Segmented
            label={t("crypto.signDefault")}
            value={settings.sign_by_default ?? "off"}
            onChange={(v) => onChange({ sign_by_default: v === "off" ? null : (v as "pgp" | "smime") })}
            options={[
              ["off", t("crypto.signOff")],
              ["pgp", "OpenPGP"],
              ["smime", "S/MIME"],
            ]}
          />
        </Row>
      </div>
      {dialog === "pgp" && <ImportKeyDialog onDone={load} onClose={() => setDialog(null)} />}
      {dialog === "smime" && <SmimeDialog onDone={load} onClose={() => setDialog(null)} />}
    </div>
  );
}
