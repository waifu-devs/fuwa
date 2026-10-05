import { useSyncExternalStore } from "react";
import { create } from "@bufbuild/protobuf";
import { AttachmentSchema, type Attachment } from "@/gen/fuwa/v1/types_pb";
import { runCancelable, uploadAttachment } from "@/fuwa/actions";
import { cantAdd, localLook, MAX_FILES, type FileLook } from "@/lib/attachments";
import { reportError, reportTiming } from "@/lib/reports";
import { toast } from "@/lib/ui";
import { i18n } from "@/i18n/i18n";

/**
 * Files picked for the next message in each channel. Each starts uploading
 * the moment it's added, so sending is quick; they stay with their channel
 * when you look elsewhere, like the text does.
 */
export type Staged = {
  id: string;
  file: File;
  look: FileLook;
  /** A local picture of it to show while it goes up (pictures only). */
  preview: string | null;
  /** How much has gone, 0 to 1. */
  sent: number;
  /** Uploaded, ready to send. */
  done: Attachment | null;
  failed: string | null;
  cancel: () => void;
};

const NONE: Staged[] = [];
const byChannel = new Map<string, Staged[]>();
const listeners = new Set<() => void>();
let next = 0;

function set(channelId: string, fn: (list: Staged[]) => Staged[]) {
  const list = fn(byChannel.get(channelId) ?? NONE);
  if (list.length) byChannel.set(channelId, list);
  else byChannel.delete(channelId);
  for (const listener of listeners) listener();
}

function patch(channelId: string, id: string, change: Partial<Staged>) {
  set(channelId, (list) => list.map((s) => (s.id === id ? { ...s, ...change } : s)));
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => listeners.delete(listener);
};

export function useStaged(channelId: string): Staged[] {
  return useSyncExternalStore(subscribe, () => byChannel.get(channelId) ?? NONE);
}

/** A picture's or video's size in pixels, so it keeps its place in the list before it loads. */
async function measure(file: File, look: FileLook): Promise<{ width: number; height: number }> {
  try {
    if (look === "picture") {
      const bitmap = await createImageBitmap(file);
      const size = { width: bitmap.width, height: bitmap.height };
      bitmap.close();
      return size;
    }
    if (look === "video") {
      const url = URL.createObjectURL(file);
      try {
        return await new Promise((resolve, reject) => {
          const video = document.createElement("video");
          video.preload = "metadata";
          video.onloadedmetadata = () => resolve({ width: video.videoWidth, height: video.videoHeight });
          video.onerror = () => reject(new Error("no metadata"));
          video.src = url;
        });
      } finally {
        URL.revokeObjectURL(url);
      }
    }
  } catch {
    // Shown in a box of its own until it loads.
  }
  return { width: 0, height: 0 };
}

function start(key: string, serverId: string, channelId: string, staged: Staged) {
  const started = performance.now();
  const upload = runCancelable(
    uploadAttachment(key, serverId, staged.file, (sent) => patch(channelId, staged.id, { sent })),
  );
  patch(channelId, staged.id, { cancel: upload.cancel, failed: null, sent: 0 });
  void Promise.all([upload.done, measure(staged.file, staged.look)]).then(
    ([url, size]) => {
      reportTiming("upload.attachment", performance.now() - started);
      const done = create(AttachmentSchema, { url, filename: staged.file.name || "file", ...size });
      patch(channelId, staged.id, { done, sent: 1 });
    },
    (err: Error) => {
      if (!(byChannel.get(channelId) ?? NONE).some((s) => s.id === staged.id)) return;
      reportError("upload_failed", "attachment");
      const message = err.message || i18n().t("system.upload.failed");
      patch(channelId, staged.id, { failed: message.replace(/^./, (c) => c.toUpperCase()) });
    },
  );
}

/** Adds files to the next message here and starts sending them up. */
export function addFiles(key: string, serverId: string, channelId: string, files: File[]) {
  if (!files.length) return;
  const already = (byChannel.get(channelId) ?? NONE).length;
  const why = cantAdd(i18n().t, already, files.length);
  if (why) toast(why);
  const room = Math.max(0, MAX_FILES - already);
  for (const file of files.slice(0, room)) {
    const look = localLook(file);
    const staged: Staged = {
      id: `f${++next}`,
      file,
      look,
      preview: look === "picture" ? URL.createObjectURL(file) : null,
      sent: 0,
      done: null,
      failed: null,
      cancel: () => {},
    };
    set(channelId, (list) => [...list, staged]);
    start(key, serverId, channelId, staged);
  }
}

export function retryFile(key: string, serverId: string, channelId: string, id: string) {
  const staged = (byChannel.get(channelId) ?? NONE).find((s) => s.id === id);
  if (staged) start(key, serverId, channelId, staged);
}

/** Takes a file back out (stopping its upload); the server clears what was sent of it. */
export function removeFile(channelId: string, id: string) {
  const staged = (byChannel.get(channelId) ?? NONE).find((s) => s.id === id);
  if (!staged) return;
  staged.cancel();
  if (staged.preview) URL.revokeObjectURL(staged.preview);
  set(channelId, (list) => list.filter((s) => s.id !== id));
}

/** The uploaded files, for the message being sent, leaving the channel's tray empty. */
export function takeFiles(channelId: string): Attachment[] {
  const list = byChannel.get(channelId) ?? NONE;
  for (const staged of list) if (staged.preview) URL.revokeObjectURL(staged.preview);
  set(channelId, () => NONE);
  return list.flatMap((s) => (s.done ? [s.done] : []));
}
