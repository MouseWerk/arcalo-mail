// Smaller fixes: Shift ranges over pages never loaded, very long folders, e-mail links in mail,
// initials outside the BMP, the shortcut table per system, Enter on controls, menus and the
// palette with the keyboard, collapsing a message card, a contact card that cannot load,
// confirmations before deleting, and text in either direction.
import { describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { clickSelect, isControl, MAX_LIST_PX, rangeGaps, scrollWindow } from "../lib/listing";
import { mailtoFromHref } from "../lib/mailto";
import { initials } from "../lib/format";
import { SHORTCUTS, keyLabel, usesCommand } from "../lib/shortcuts";
import { api } from "../lib/api";
import { Menu } from "../components/ui";
import { CommandPalette } from "../components/CommandPalette";
import { ContactCardDialog } from "../components/ContactCard";
import { TemplatesSection } from "../components/PowerSettings";
import { Mail } from "lucide-react";
import { click, flush, key, mounted, mountApp } from "./harness";

async function render(node: React.ReactNode) {
  const el = document.createElement("div");
  document.body.appendChild(el);
  const root = createRoot(el);
  mounted.push(root);
  await act(async () => root.render(node));
  return el;
}

describe("a Shift range", () => {
  it("names the pages it lacks, and has no hole once they are there", () => {
    const order = Array.from({ length: 300 }, (_, i) => (i >= 100 && i < 200 ? undefined : 1000 + i));
    const first = clickSelect({ ids: [], anchor: null }, order, 50, { shift: false, toggle: false });
    expect(rangeGaps(first, order, 250, 100)).toEqual([1]);
    const full = order.map((id, i) => id ?? 1000 + i);
    expect(rangeGaps(first, full, 250, 100)).toEqual([]);
    expect(clickSelect(first, full, 250, { shift: true, toggle: false }).ids.length).toBe(201);
  });
});

describe("a very long folder", () => {
  it("stays within what a browser draws and reaches its last row", () => {
    const total = 1_000_000;
    const w = scrollWindow(0, 800, 76, total);
    expect(w.height).toBe(MAX_LIST_PX);
    const end = scrollWindow(MAX_LIST_PX - 800, 800, 76, total);
    expect(end.end).toBe(total);
    // The last row is drawn at the bottom of the viewport.
    expect((total - 1) * 76 - end.shift + 76).toBeCloseTo(MAX_LIST_PX, 0);
  });

  it("is drawn as it is when it fits", () => {
    const w = scrollWindow(760, 800, 76, 1000);
    expect(w.shift).toBe(0);
    expect(w.ratio).toBe(1);
    expect(w.height).toBe(76_000);
  });
});

describe("mailto links in a message", () => {
  it("take subject, text, Cc and several addresses", () => {
    const m = mailtoFromHref("mailto:a@example.org,b@example.org?cc=c%40example.org&subject=Gr%C3%BC%C3%9Fe&body=Hallo%0D%0Azwei&from=x@evil.example")!;
    expect(m.to).toEqual(["a@example.org", "b@example.org"]);
    expect(m.cc).toEqual(["c@example.org"]);
    expect(m.subject).toBe("Grüße");
    expect(m.body).toBe("Hallo\nzwei");
  });

  it("survive a broken escape", () => {
    expect(mailtoFromHref("mailto:a@example.org?subject=100%")!.subject).toBe("100%");
    expect(mailtoFromHref("https://example.org")).toBeNull();
  });

  it("open the compose sheet with all of it", async () => {
    const { el } = await mountApp();
    await act(async () => {
      window.dispatchEvent(new CustomEvent("arcalo-mail:mailto", { detail: "mailto:a@example.org,b@example.org?subject=Hallo%20du&body=Text" }));
    });
    await flush(50);
    expect((el.querySelector("#compose-subject") as HTMLInputElement).value).toBe("Hallo du");
    expect(el.querySelectorAll(".compose .chip:not(.chip-file)").length).toBe(2);
    expect(el.querySelector(".compose .editor")?.textContent).toContain("Text");
  });
});

describe("initials", () => {
  it("keep a letter outside the BMP whole", () => {
    expect(initials({ name: "𝒜nna 𝒮chmidt", email: "a@example.org" })).toBe("𝒜𝒮");
    expect(initials({ name: "Jonas Berger", email: "j@example.org" })).toBe("JB");
  });
});

describe("the shortcut table", () => {
  const names = { ctrl: "Strg", shift: "Umschalt", enter: "Eingabe", del: "Entf" };
  it("says ⌘ on macOS and Strg elsewhere", () => {
    const send = SHORTCUTS.find((s) => s.label === "keys.send")!;
    expect(send.keys[0].map((k) => keyLabel(k, usesCommand("macos"), names)).join("")).toBe("⌘↩");
    expect(send.keys[0].map((k) => keyLabel(k, usesCommand("windows"), names)).join("+")).toBe("Strg+Eingabe");
  });

  it("lists o, the more menu, Delete and Ctrl/Cmd+N", () => {
    const all = SHORTCUTS.flatMap((s) => s.keys.map((k) => k.join("+")));
    for (const k of ["o", ".", "Delete", "Mod+N"]) expect(all).toContain(k);
  });
});

describe("Enter on controls", () => {
  it("belongs to buttons and tabs outside the list, to the list inside it", () => {
    const wrap = document.createElement("div");
    wrap.innerHTML = '<button id="b">x</button><div class="messages" tabindex="0"><div id="r" role="option">row</div></div><span id="s">t</span>';
    document.body.appendChild(wrap);
    expect(isControl(wrap.querySelector("#b"))).toBe(true);
    expect(isControl(wrap.querySelector("#r"))).toBe(false);
    expect(isControl(wrap.querySelector(".messages"))).toBe(false);
    expect(isControl(wrap.querySelector("#s"))).toBe(false);
  });
});

describe("menus", () => {
  it("go to the first and last item with Home and End, close on Tab and give the focus back", async () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    const onClose = vi.fn();
    await render(
      <Menu
        label="Aktionen"
        at={{ x: 0, y: 0 }}
        onClose={onClose}
        items={[
          { label: "Eins", onSelect: () => {} },
          { label: "Zwei", onSelect: () => {} },
          { label: "Drei", onSelect: () => {} },
        ]}
      />,
    );
    // The menu is drawn on the page itself (a portal), above dialogs.
    const items = Array.from(document.querySelectorAll<HTMLElement>(".menu [role=menuitem]"));
    expect(document.activeElement).toBe(items[0]);
    await key(items[0], "End");
    expect(document.activeElement).toBe(items[2]);
    await key(items[2], "Home");
    expect(document.activeElement).toBe(items[0]);
    await key(items[0], "Tab");
    expect(onClose).toHaveBeenCalled();
    await act(async () => mounted.pop()!.unmount());
    expect(document.activeElement).toBe(opener);
  });

  it("the palette keeps Tab in its field and gives the focus back", async () => {
    const opener = document.createElement("button");
    document.body.appendChild(opener);
    opener.focus();
    const el = await render(<CommandPalette commands={[{ id: "a", label: "Neue Nachricht", icon: Mail, run: () => {} }]} search={() => ({ id: "s", label: "Suchen", icon: Mail, run: () => {} })} onClose={() => {}} />);
    const input = el.querySelector<HTMLInputElement>(".palette-input")!;
    expect(document.activeElement).toBe(input);
    const e = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    await act(async () => {
      input.dispatchEvent(e);
    });
    expect(e.defaultPrevented).toBe(true);
    await act(async () => mounted.pop()!.unmount());
    expect(document.activeElement).toBe(opener);
  });
});

describe("a conversation", () => {
  it("collapses a message with a button the keyboard reaches", async () => {
    const { el } = await mountApp();
    await click(el.querySelector('.row[data-id="3"]')!);
    await flush(100);
    // Expand the older message too, then collapse it with its button.
    await click(el.querySelector(".message-collapsed")!);
    await flush(100);
    expect(el.querySelectorAll(".reading-pane article.message-card").length).toBe(2);
    const collapse = el.querySelector<HTMLButtonElement>('.reading-pane article.message-card [aria-label="Einklappen"]')!;
    expect(collapse.tagName).toBe("BUTTON");
    await click(collapse);
    await flush(50);
    expect(el.querySelectorAll(".reading-pane article.message-card").length).toBe(1);
  });
});

describe("the contact card", () => {
  it("says so when it cannot load instead of spinning", async () => {
    vi.spyOn(api, "contactCard").mockRejectedValue("database is locked");
    const el = await render(<ContactCardDialog email="a@example.org" name="Anna" onClose={() => {}} onCompose={() => {}} onOpen={() => {}} onSave={() => {}} />);
    await flush(20);
    expect(el.querySelector(".contact-loading")).toBeNull();
    expect(el.querySelector(".contact-error")?.textContent).toContain("database is locked");
  });
});

describe("deleting a template", () => {
  it("asks first", async () => {
    vi.spyOn(api, "templates").mockResolvedValue([{ id: 7, kind: "template", name: "Angebot", trigger: null, subject: "Angebot", body: "Hallo", account_id: null }]);
    const del = vi.spyOn(api, "templateDelete").mockResolvedValue(undefined);
    const el = await render(<TemplatesSection accounts={[]} />);
    await flush(20);
    await click(el.querySelector('[aria-label="Angebot löschen"]')!);
    expect(del).not.toHaveBeenCalled();
    const dialog = el.querySelector(".dialog")!;
    expect(dialog.textContent).toContain("Angebot");
    await click(dialog.querySelector(".btn-danger")!);
    await flush(20);
    expect(del).toHaveBeenCalledWith(7);
  });
});

describe("text in either direction", () => {
  it("isolates names, subjects and snippets in the list and the subject in the sheet", async () => {
    const { el } = await mountApp();
    const row = el.querySelector('.row[data-id="1"]')!;
    expect(row.querySelector("bdi.row-from")).not.toBeNull();
    expect(row.querySelector(".row-subject bdi")).not.toBeNull();
    await key(document.body, "c");
    await flush(50);
    expect(el.querySelector("#compose-subject")?.getAttribute("dir")).toBe("auto");
    expect(el.querySelector(".compose .editor")?.getAttribute("dir")).toBe("auto");
  });
});
