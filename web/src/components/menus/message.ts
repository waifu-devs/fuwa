import { CopyIcon, DownloadIcon, ExternalLinkIcon, ImageIcon, LinkIcon, MessageSquareReplyIcon, PencilIcon, TextSelectIcon, Trash2Icon, UserXIcon } from "lucide-react";
import type { MenuTrigger } from "@/components/ContextMenu";
import { copyIdItem } from "@/components/menus/common";
import { i18n } from "@/i18n/i18n";
import { items, withExtensions, type MenuContexts, type MenuSection } from "@/lib/context-menu";
import { reportError } from "@/lib/reports";
import { shownPicture } from "@/lib/shown";
import { HIDDEN_ADDRESS, hidesPersonal } from "@/lib/streamer";
import { copy, toast } from "@/lib/ui";

/** Where a link goes, by host, so you can tell before opening it. */
const hostOf = (href: string) => {
  try {
    return new URL(href).host;
  } catch {
    return "";
  }
};

/** Where a link goes, to show by it; an instance's own address hides in streamer mode, like everywhere else. */
const linkHint = (href: string) => (hidesPersonal() && shownPicture(href) ? HIDDEN_ADDRESS : hostOf(href));

/** A name for a saved picture: the last part of its address, with an extension for its type. */
function fileName(src: string, type: string) {
  const last = hostOf(src) ? new URL(src).pathname.split("/").pop() || "picture" : "picture";
  const base = last.replace(/[^\w.-]+/g, "_").slice(0, 80) || "picture";
  const ext = type.startsWith("image/") ? type.slice(6).replace("jpeg", "jpg").replace(/\+.*/, "") : "";
  return /\.\w{2,5}$/.test(base) || !ext ? base : `${base}.${ext}`;
}

/** Saves a picture as a file. Pictures only ever come from fuwa instances, so this asks one of them, nobody else. */
async function savePicture(src: string) {
  try {
    const res = await fetch(src, { credentials: "omit" });
    if (!res.ok) throw new Error("not saved");
    const blob = await res.blob();
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = fileName(src, blob.type);
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 10_000);
  } catch {
    // Some instances don't let another site read their pictures: open it, to save from there.
    reportError("context_menu.save_picture", "message");
    toast("Opened the picture to save from there");
    window.open(src, "_blank", "noopener,noreferrer");
  }
}

/**
 * What was right-clicked inside something: selected text, a link or a
 * picture. Comes first in the menu, as browsers put it.
 */
export function targetSection(trigger: MenuTrigger): MenuSection {
  const { target, element, selection } = trigger;
  const link = target.closest<HTMLAnchorElement>("a[href]");
  const picture = target.closest<HTMLImageElement>("img[src]");
  const href = link && element.contains(link) ? link.href : "";
  // Only pictures from instances the app already talks to: opening or saving one asks nobody else.
  const src = picture && element.contains(picture) ? shownPicture(picture.src) : "";
  const { t } = i18n();
  return {
    id: "target",
    items: items(
      !!selection && { id: "copy-selection", label: "Copy", icon: TextSelectIcon, onSelect: () => copy(t, selection, t("common.copy.text")) },
      !!href && { id: "open-link", label: "Open link", icon: ExternalLinkIcon, hint: linkHint(href), onSelect: () => link?.click() },
      !!href && { id: "copy-link", label: "Copy link", icon: LinkIcon, onSelect: () => copy(t, href, t("common.copy.link")) },
      !!src && { id: "open-picture", label: "Open picture", icon: ImageIcon, onSelect: () => window.open(src, "_blank", "noopener,noreferrer") },
      !!src && { id: "save-picture", label: "Save picture", icon: DownloadIcon, onSelect: () => void savePicture(src) },
      !!src && { id: "copy-picture-link", label: "Copy picture link", icon: LinkIcon, onSelect: () => copy(t, src, t("common.copy.pictureLink")) },
    ),
  };
}

/** What a message's own buttons can do, as menu items; each is there only when its button is. */
export type MessageMenuActions = {
  /** Its thread: start one, or open the one it has. */
  thread?: { open: boolean; go: () => void };
  edit?: () => void;
  copyText?: () => void;
  keepOut?: { name: string; ask: () => void };
  delete?: () => void;
};

/**
 * A message's menu. Sections, in order: target (selection, link, picture),
 * react, primary, manage, developer, danger. Reactions, replies, pins and
 * reports join through `extendMenu("message", …)` once they exist.
 */
export function messageMenu(ctx: MenuContexts["message"], trigger: MenuTrigger, actions: MessageMenuActions): MenuSection[] {
  return [
    targetSection(trigger),
    ...withExtensions("message", ctx, [
      { id: "react", items: [] },
      {
        id: "primary",
        items: items(
          actions.thread && { id: "thread", label: actions.thread.open ? "Open thread" : "Reply in thread", icon: MessageSquareReplyIcon, onSelect: actions.thread.go },
          actions.edit && { id: "edit", label: "Edit message", icon: PencilIcon, onSelect: actions.edit },
          actions.copyText && { id: "copy-text", label: "Copy text", icon: CopyIcon, onSelect: actions.copyText },
        ),
      },
      {
        id: "manage",
        items: items(actions.keepOut && { id: "keep-out", label: `Keep ${actions.keepOut.name} out`, icon: UserXIcon, danger: true, onSelect: actions.keepOut.ask }),
      },
      { id: "developer", items: items(copyIdItem(ctx.message.id, "message")) },
      { id: "danger", items: items(actions.delete && { id: "delete", label: "Delete message", icon: Trash2Icon, danger: true, onSelect: actions.delete }) },
    ]),
  ].filter((s) => s.items.length > 0);
}
