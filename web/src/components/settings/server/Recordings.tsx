import { AudioLinesIcon, MonitorIcon, VideoIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { getRecordingVideo, run, updateServer } from "@/fuwa/actions";
import { useAction } from "@/fuwa/hooks";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { Segmented } from "@/components/settings/account/common";
import { Choice, SaveBar, WithPreview } from "@/components/settings/controls";
import { CEILING_FRAME_RATES, CEILING_HEIGHTS } from "@/lib/camera-quality";
import { useI18n } from "@/i18n/react";

/**
 * Calls on the server: a ceiling on cameras in its voice channels (none
 * unless someone sets one, so everyone sends their best), then what
 * recordings on the server keep: everyone's sound, or their cameras
 * and shared screens too, where the instance lets servers keep video.
 * Changing it ends the recording going on; the next starts when someone
 * presses Record.
 */
export function RecordingSettings({ instanceKey, server }: { instanceKey: string; server: Server }) {
  const { t } = useI18n();
  const [video, setVideo] = useState(server.recordVideo);
  const [height, setHeight] = useState(server.cameraMaxHeight);
  const [fps, setFps] = useState(server.cameraMaxFps);
  const save = useAction(updateServer);
  const changed = video !== server.recordVideo;
  const count = [changed, height !== server.cameraMaxHeight, fps !== server.cameraMaxFps].filter(Boolean).length;
  const none = t("serversettings.camera.none");
  // Until the instance says, it may: saving would say if not.
  const [allowed, setAllowed] = useState(true);
  useEffect(() => {
    run(getRecordingVideo(instanceKey)).then(setAllowed, () => setAllowed(true));
  }, [instanceKey]);

  return (
    <WithPreview preview={<FilesPreview video={video} />}>
      <div className="flex flex-col">
        <div data-setting="camera-quality" className="flex flex-col gap-3 pb-6">
          <span>
            <span className="block font-extrabold">{t("serversettings.camera.title")}</span>
            <span className="block text-sm text-muted-foreground">{t("serversettings.camera.hint")}</span>
          </span>
          <div className="flex flex-col gap-1.5">
            <span className="text-sm font-bold">{t("serversettings.camera.resolution")}</span>
            <Segmented
              label={t("serversettings.camera.resolution")}
              value={height}
              onChange={setHeight}
              options={CEILING_HEIGHTS.map((n) => ({ value: n, label: n ? t("appsettings.voice.heightValue", { height: n }) : none }))}
              className="flex w-full max-w-md"
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <span className="text-sm font-bold">{t("serversettings.camera.fps")}</span>
            <Segmented
              label={t("serversettings.camera.fps")}
              value={fps}
              onChange={setFps}
              options={CEILING_FRAME_RATES.map((n) => ({ value: n, label: n ? t("appsettings.voice.fpsValue", { fps: n }) : none }))}
              className="flex w-full max-w-md"
            />
          </div>
        </div>
        <div data-setting="record-video" className="flex flex-col gap-3 pb-5">
          <span>
            <span className="block font-extrabold">{t("serversettings.recordings.title")}</span>
            <span className="block text-sm text-muted-foreground">{t("serversettings.recordings.hint")}</span>
          </span>
          <Choice
            value={video ? "video" : "sound"}
            onChange={(v) => setVideo(v === "video")}
            options={[
              { value: "sound", label: t("serversettings.recordings.sound"), hint: t("serversettings.recordings.soundHint"), icon: <AudioLinesIcon className="size-4" /> },
              {
                value: "video",
                label: t("serversettings.recordings.video"),
                hint: t("serversettings.recordings.videoHint"),
                icon: <VideoIcon className="size-4" />,
                disabled: allowed || server.recordVideo ? undefined : t("serversettings.recordings.videoOff"),
              },
            ]}
          />
          <AnimatePresence mode="popLayout" initial={false}>
            {!allowed && !server.recordVideo && (
              <motion.p {...SLIDE_IN} transition={SPRING} className="text-xs text-muted-foreground">
                {t("serversettings.recordings.videoOffHint")}
              </motion.p>
            )}
          </AnimatePresence>
          <AnimatePresence mode="popLayout" initial={false}>
            {changed && (
              <motion.p layout="position" {...SLIDE_IN} transition={SPRING} className="text-xs text-amber-600 dark:text-amber-400">
                {video ? t("serversettings.recordings.changedVideo") : t("serversettings.recordings.changedSound")}
              </motion.p>
            )}
          </AnimatePresence>
        </div>
      </div>
      <SaveBar
        count={count}
        saving={save.pending}
        error={save.error}
        onSave={() =>
          void save.go(instanceKey, server.id, {
            ...(changed ? { recordVideo: video } : {}),
            ...(height !== server.cameraMaxHeight ? { cameraMaxHeight: height } : {}),
            ...(fps !== server.cameraMaxFps ? { cameraMaxFps: fps } : {}),
          })
        }
        onDiscard={() => {
          setVideo(server.recordVideo);
          setHeight(server.cameraMaxHeight);
          setFps(server.cameraMaxFps);
          save.setError(null);
        }}
      />
    </WithPreview>
  );
}

/** One person's files from an hour of recording, as the setting would keep them. */
function FilesPreview({ video }: { video: boolean }) {
  const { t } = useI18n();
  const files = [
    { key: "sound", name: "Mika.opus", icon: AudioLinesIcon, size: "4 MB" },
    ...(video
      ? [
          { key: "camera", name: "Mika camera.webm", icon: VideoIcon, size: "90 MB" },
          { key: "screen", name: "Mika screen.webm", icon: MonitorIcon, size: "60 MB" },
        ]
      : []),
  ];
  return (
    <div className="flex flex-col gap-2 rounded-2xl border bg-muted/40 p-3">
      <p className="text-xs text-muted-foreground">{t("serversettings.recordings.preview")}</p>
      <ul className="flex flex-col gap-1.5">
        <AnimatePresence initial={false} mode="popLayout">
          {files.map((file, n) => (
            <motion.li
              key={file.key}
              layout
              initial={{ opacity: 0, x: 16, scale: 0.95 }}
              animate={{ opacity: 1, x: 0, scale: 1, transition: { ...SPRING, delay: n * 0.06 } }}
              exit={{ opacity: 0, x: -16, scale: 0.95, transition: { duration: 0.15 } }}
              className="flex items-center gap-2 rounded-xl bg-background/70 px-2.5 py-2"
            >
              <span className="grid size-7 shrink-0 place-items-center rounded-lg bg-primary/10 text-primary">
                <file.icon className="size-4" />
              </span>
              <span className="min-w-0 flex-1 truncate text-sm font-bold">{file.name}</span>
              <span className="shrink-0 text-xs text-muted-foreground tabular-nums">~{file.size}</span>
            </motion.li>
          ))}
        </AnimatePresence>
      </ul>
    </div>
  );
}
