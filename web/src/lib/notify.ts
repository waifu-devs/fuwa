import { timestampDate } from "@bufbuild/protobuf/wkt";
import type { Event } from "@/gen/fuwa/v1/types_pb";
import { store, type InstanceState } from "@/fuwa/store";
import { displayName, memberName, mentions } from "@/lib/format";
import { effectiveNotifications, mentionsEveryone, shouldAlert } from "@/lib/notifications";
import { getPrefs, subscribePrefs } from "@/lib/prefs";
import { play } from "@/lib/sounds";

/**
 * What this device does when something happens while you're elsewhere:
 * sounds, desktop notifications, and the unread count in the tab's title
 * and icon. All of it follows the app settings, and streamer mode can mute
 * sounds and notifications.
 */

type Open = (instance: string, server: string, channel: string) => void;
let openChannel: Open | null = null;

/** Where clicking a notification goes; set once the router is up. */
export const setNotificationTarget = (open: Open) => {
  openChannel = open;
};

/** Events older than this came in while catching up after a reconnect; they stay quiet. */
const FRESH_MS = 30_000;
const lastSound = { message: 0, mention: 0, join: 0 };

function playSome(sound: keyof typeof lastSound, gap: number) {
  const now = Date.now();
  if (now - lastSound[sound] < gap) return;
  lastSound[sound] = now;
  play(sound);
}

/** Called for each event as it arrives live. */
export function onLiveEvent(key: string, event: Event) {
  if (event.createdAt && Date.now() - timestampDate(event.createdAt).getTime() > FRESH_MS) return;
  const s = store.get();
  const inst = s.instances[key];
  const me = inst?.me;
  if (!inst || !me) return;
  const p = event.payload;
  if (p.case === "messageCreated") {
    const message = p.value.message;
    if (!message || message.authorId === me.id) return;
    const looking = !document.hidden && s.focus?.instance === key && s.focus.channel === message.channelId;
    const settings = effectiveNotifications(inst, event.serverId, message.channelId);
    const mention =
      mentions(message.content, me.username) ||
      (!settings.suppressEveryone && mentionsEveryone(inst, event.serverId, message.authorId, message.content));
    const alert = shouldAlert(settings, mention, getPrefs());
    if (alert.sound && !looking) playSome(mention ? "mention" : "message", mention ? 600 : 1500);
    if (!alert.notify || (!document.hidden && document.hasFocus())) return;
    notify(inst, event.serverId, message.channelId, message.authorId, message.content, mention);
  } else if (p.case === "memberJoined") {
    const user = p.value.member?.user;
    const viewing = s.focus?.instance === key && (inst.channels[event.serverId] ?? []).some((c) => c.id === s.focus!.channel);
    if (user && user.id !== me.id && viewing) playSome("join", 800);
  }
}

function notify(inst: InstanceState, serverId: string, channelId: string, authorId: string, content: string, mention: boolean) {
  const p = getPrefs();
  if (!p.desktopNotifications || (p.streamer && p.streamerMuteNotifications)) return;
  if (typeof Notification === "undefined" || Notification.permission !== "granted") return;
  const channel = inst.channels[serverId]?.find((c) => c.id === channelId);
  const member = inst.members[serverId]?.find((m) => m.user?.id === authorId);
  const author = member ? memberName(member) : displayName(inst.users[authorId]);
  const body = content.replace(/[*_~`>#]+/g, "").replace(/\s+/g, " ").trim();
  try {
    const n = new Notification(`${author}${channel ? ` in #${channel.name}` : ""}`, {
      body: `${mention ? "Mentioned you: " : ""}${body.length > 160 ? `${body.slice(0, 159)}…` : body}`,
      icon: "/favicon.svg",
      tag: channelId,
    });
    n.onclick = () => {
      window.focus();
      openChannel?.(inst.key, serverId, channelId);
      n.close();
    };
  } catch {
    // Some browsers only allow notifications from a service worker; stay quiet there.
  }
}

/** Shows a sample notification, for the settings page. */
export function testNotification() {
  if (typeof Notification === "undefined" || Notification.permission !== "granted") return false;
  try {
    new Notification("fuwa", { body: "This is how messages will reach you ✨", icon: "/favicon.svg", tag: "fuwa-test" });
    return true;
  } catch {
    return false;
  }
}

// ───────────────────────── Tab title and icon ─────────────────────────

let baseTitle = "fuwa";
let badge = 0;

/** Sets the page's title; the unread count goes in front of it. */
export function setTitle(title: string) {
  baseTitle = title;
  document.title = (badge ? `(${badge > 99 ? "99+" : badge}) ` : "") + baseTitle;
}

function unreadTotal(): number {
  if (!getPrefs().unreadBadge) return 0;
  let total = 0;
  const now = Date.now();
  for (const inst of Object.values(store.get().instances)) {
    for (const [serverId, channels] of Object.entries(inst.channels)) {
      for (const channel of channels) {
        const n = inst.unread[channel.id];
        if (n && !effectiveNotifications(inst, serverId, channel.id, now).muted) total += n;
      }
    }
  }
  return total;
}

let icon: HTMLImageElement | null = null;

function drawIcon(count: number) {
  const link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
  if (!link) return;
  if (!count) {
    link.type = "image/svg+xml";
    link.href = "/favicon.svg";
    return;
  }
  const paint = () => {
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 64;
    const g = canvas.getContext("2d");
    if (!g || !icon) return;
    g.drawImage(icon, 0, 0, 64, 64);
    g.beginPath();
    g.arc(46, 18, 17, 0, Math.PI * 2);
    g.fillStyle = "#e5484d";
    g.fill();
    g.lineWidth = 4;
    g.strokeStyle = "#ffffff";
    g.stroke();
    g.fillStyle = "#ffffff";
    g.font = "800 22px system-ui, sans-serif";
    g.textAlign = "center";
    g.textBaseline = "middle";
    g.fillText(count > 9 ? "9+" : String(count), 46, 19);
    link.type = "image/png";
    link.href = canvas.toDataURL("image/png");
  };
  if (icon?.complete) return paint();
  icon = new Image();
  icon.onload = paint;
  icon.src = "/favicon.svg";
}

/** Keeps the tab's title and icon showing the unread count. */
export function watchUnread() {
  const update = () => {
    const next = unreadTotal();
    if (next === badge) return;
    badge = next;
    setTitle(baseTitle);
    drawIcon(next);
  };
  store.subscribe(update);
  subscribePrefs(update);
  // Timed mutes run out on their own.
  setInterval(update, 30_000);
  update();
}
