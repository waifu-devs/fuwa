import { timestampDate } from "@bufbuild/protobuf/wkt";
import { LeaveReason, MessageKind, type Event, type User } from "@/gen/fuwa/v1/types_pb";
import { store, type InstanceState } from "@/fuwa/store";
import { displayName, memberName } from "@/lib/format";
import { effectiveNotifications, pingsMe, shouldAlert } from "@/lib/notifications";
import { getPrefs, subscribePrefs } from "@/lib/prefs";
import { play } from "@/lib/sounds";
import { toast } from "@/lib/ui";

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

type OpenDm = (instance: string, conversation: string) => void;
let openDm: OpenDm | null = null;

/** Where clicking a direct message's notification goes. */
export const setDmNotificationTarget = (open: OpenDm) => {
  openDm = open;
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
    // Join messages chime through memberJoined instead.
    if (!message || message.authorId === me.id || message.kind !== MessageKind.UNSPECIFIED) return;
    const looking = !document.hidden && s.focus?.instance === key && s.focus.channel === message.channelId;
    const settings = effectiveNotifications(inst, event.serverId, message.channelId);
    const mention = pingsMe(inst, event.serverId, message, settings.suppressEveryone);
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

/**
 * A direct message this device just opened: it chimes like a mention and,
 * while you're elsewhere, shows on the desktop. The text only ever goes to
 * this device's own notifications.
 */
export function onDirectMessage(key: string, conversationId: string, author: User | undefined, content: string, at: number) {
  if (Date.now() - at > FRESH_MS) return;
  const s = store.get();
  const inst = s.instances[key];
  if (!inst?.me || author?.id === inst.me.id) return;
  const looking = !document.hidden && s.focus?.instance === key && s.focus.channel === conversationId;
  if (!looking) playSome("mention", 600);
  if (!document.hidden && document.hasFocus()) return;
  const p = getPrefs();
  if (!p.desktopNotifications || (p.streamer && p.streamerMuteNotifications)) return;
  if (typeof Notification === "undefined" || Notification.permission !== "granted") return;
  const body = content.replace(/[*_~`>#]+/g, "").replace(/\s+/g, " ").trim();
  try {
    const n = new Notification(displayName(author), {
      body: body.length > 160 ? `${body.slice(0, 159)}…` : body,
      icon: "/favicon.svg",
      tag: conversationId,
    });
    n.onclick = () => {
      window.focus();
      openDm?.(key, conversationId);
      n.close();
    };
  } catch {
    // Some browsers only allow notifications from a service worker; stay quiet there.
  }
}

/** Tells you when an owner or admin took you out of a server. Called with its name, which is gone from the store by then. */
export function onRemoved(serverName: string, reason: LeaveReason) {
  if (reason === LeaveReason.KICKED) toast(`You were removed from ${serverName}`);
  else if (reason === LeaveReason.BANNED) toast(`You were banned from ${serverName}`);
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
    for (const n of Object.values(inst.dms.unread)) total += n;
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
