// The Android app on an emulator (or a phone on USB), driven over adb and the WebView's DevTools
// socket: the screen size and density set with `wm`, the app started fresh, the test server on
// the host reached through `adb reverse` (the app sees it on 127.0.0.1, so the plain-text
// connections stay on loopback as the app allows them), and the page controlled over the
// DevTools protocol (cdp.js) after `adb forward tcp:<port> localabstract:webview_devtools_remote_<pid>`.
// Only processes this file started are stopped.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { Page } from "./cdp.js";

export const PKG = "de.mousewerk.arcalo.mail";
const ADB = process.env.ADB ?? "adb";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export function adb(...args) {
  return execFileSync(ADB, args, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
}

export const shell = (cmd) => adb("shell", cmd);

/** Installs the APK (again). */
export function install(apk) {
  adb("install", "-r", "-t", "-g", apk);
}

/** The host's ports on the device's loopback. */
export function reverse(ports) {
  for (const p of ports) adb("reverse", `tcp:${p}`, `tcp:${p}`);
}

/** A phone of `w`×`h` dp at `density` (dpi / 160): `wm size` in pixels and `wm density`. */
export function setScreen(w, h, density = 2) {
  shell(`wm size ${Math.round(w * density)}x${Math.round(h * density)}`);
  shell(`wm density ${Math.round(density * 160)}`);
}

export function resetScreen() {
  shell("wm size reset");
  shell("wm density reset");
}

/** The app from its first start: data cleared, notifications allowed (no system dialog over
 *  the app), the soft keyboard shown although the emulator has a hardware keyboard. */
export function fresh() {
  try {
    shell(`am force-stop ${PKG}`);
  } catch {
    /* not running */
  }
  shell(`pm clear ${PKG}`);
  try {
    shell(`pm grant ${PKG} android.permission.POST_NOTIFICATIONS`);
  } catch {
    /* older Android: no such permission */
  }
  shell("settings put secure show_ime_with_hard_keyboard 1");
}

export async function start(timeout = 60000) {
  shell(`am start -W -n ${PKG}/.MainActivity`);
  const t0 = Date.now();
  while (Date.now() - t0 < timeout) {
    const pid = shell(`pidof ${PKG}`).trim().split(/\s+/)[0];
    if (pid) return Number(pid);
    await sleep(300);
  }
  throw new Error("the app did not start");
}

let forwarded = null;

/** Connects to the app's page in its WebView. */
export async function connect(pid, timeout = 60000) {
  const port = 9222;
  const t0 = Date.now();
  let last = null;
  while (Date.now() - t0 < timeout) {
    try {
      const name = `webview_devtools_remote_${pid}`;
      if (!shell("cat /proc/net/unix").includes(name)) throw new Error(`no ${name} yet (is WebView debugging on in this build?)`);
      adb("forward", `tcp:${port}`, `localabstract:${name}`);
      forwarded = port;
      const version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
      const page = await Page.connect(port, /tauri\.localhost|^tauri:/);
      console.log(`connected to ${page.url()} in WebView ${version.Browser ?? "?"}`);
      return page;
    } catch (e) {
      last = e;
      await sleep(500);
    }
  }
  throw new Error(`could not connect to the WebView: ${last?.message ?? last}`);
}

export function unforward() {
  if (forwarded == null) return;
  try {
    adb("forward", "--remove", `tcp:${forwarded}`);
  } catch {
    /* gone */
  }
  forwarded = null;
}

/** The whole screen, status and navigation bar included. */
export function screencap(file) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, execFileSync(ADB, ["exec-out", "screencap", "-p"], { maxBuffer: 64 * 1024 * 1024 }));
}

export const tap = (x, y) => shell(`input tap ${Math.round(x)} ${Math.round(y)}`);
export const back = () => shell("input keyevent 4");

/** Frames of the system bars in screen pixels, from `dumpsys window` (Android 11 and later). */
export function systemBars() {
  const out = shell("dumpsys window");
  const frame = (type) => {
    // `type=statusBars frame=[…] visible=true` (Android 11, 12) or `mType=statusBars,
    // mFrame=[…], mVisible=true` (13 and later).
    const re = new RegExp(`type=${type}\\b[^\\n]*?frame=\\[(\\d+),(\\d+)\\]\\[(\\d+),(\\d+)\\][^\\n]*?visible=(true|false)`, "gi");
    const all = [...out.matchAll(re)].map((m) => ({ left: +m[1], top: +m[2], right: +m[3], bottom: +m[4], visible: m[5] === "true" }));
    return all.find((f) => f.visible && f.bottom > f.top) ?? null;
  };
  const status = frame("statusBars");
  const nav = frame("navigationBars");
  if (!status && !nav) throw new Error(`no system bars in dumpsys window:\n${out.split("\n").filter((l) => /InsetsSource|statusBars|navigationBars/.test(l)).slice(0, 20).join("\n")}`);
  return { status, nav };
}

/** Where the WebView is on the screen (pixels), from the view tree of `dumpsys activity top`:
 *  each view's frame is relative to its parent, so the frames along the way add up. */
export function webViewBounds() {
  const out = shell(`dumpsys activity top`);
  const start = out.lastIndexOf(`ACTIVITY ${PKG}`);
  const lines = out.slice(start).split("\n");
  const stack = [];
  for (const line of lines) {
    const m = line.match(/^(\s*)\S+\{[^}]*?\s(-?\d+),(-?\d+)-(-?\d+),(-?\d+)/);
    if (!m) continue;
    const indent = m[1].length;
    while (stack.length && stack[stack.length - 1].indent >= indent) stack.pop();
    const parent = stack[stack.length - 1] ?? { x: 0, y: 0 };
    const x = parent.x + Number(m[2]);
    const y = parent.y + Number(m[3]);
    const node = { indent, x, y, w: Number(m[4]) - Number(m[2]), h: Number(m[5]) - Number(m[3]) };
    stack.push(node);
    if (/WebView\{/.test(line)) return { left: x, top: y, right: x + node.w, bottom: y + node.h };
  }
  throw new Error(`no WebView in the view tree:\n${lines.slice(0, 60).join("\n")}`);
}

/** The system bars over the WebView, in CSS pixels of the page. */
export function insetsCss(dpr) {
  const bars = systemBars();
  const wv = webViewBounds();
  const over = (bar, side) => {
    if (!bar) return 0;
    const v = side === "top" ? bar.bottom - wv.top : wv.bottom - bar.top;
    return Math.max(0, v) / dpr;
  };
  return { insets: { top: over(bars.status, "top"), bottom: over(bars.nav, "bottom"), left: 0, right: 0 }, bars, webView: wv };
}

/** Whether the soft keyboard is up. */
export function keyboardShown() {
  return /mInputShown=true/.test(shell("dumpsys input_method"));
}

/** What the device says when something went wrong: the app's process, its DevTools sockets,
 *  the windows and the end of the log. */
export function diagnose(file) {
  const parts = [];
  const run = (label, f) => {
    try {
      parts.push(`== ${label}\n${f()}`);
    } catch (e) {
      parts.push(`== ${label}\n(failed: ${e.message})`);
    }
  };
  run("pidof", () => shell(`pidof ${PKG}`));
  run("devtools sockets", () => shell("cat /proc/net/unix").split("\n").filter((l) => l.includes("devtools")).join("\n"));
  run("forwards", () => adb("forward", "--list"));
  run("reverse", () => adb("reverse", "--list"));
  run("focused window", () => shell("dumpsys window windows").split("\n").filter((l) => /mCurrentFocus|mFocusedApp/.test(l)).join("\n"));
  run("logcat", () => adb("logcat", "-d", "-t", "400"));
  const text = parts.join("\n\n");
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
  console.error(text.slice(0, 6000));
}
