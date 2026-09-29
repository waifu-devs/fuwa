import { create } from "@bufbuild/protobuf";
import { AlignJustifyIcon, MessageSquareTextIcon, MonitorIcon, Rows2Icon, Rows3Icon, Rows4Icon, SnailIcon, SparklesIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { UserSchema } from "@/gen/fuwa/v1/types_pb";
import { MessageBody, MessageLine } from "@/components/chat/MessageList";
import { SPRING } from "@/components/motion";
import { Choice, Toggle, WithPreview } from "@/components/settings/controls";
import { Slider } from "@/components/ui/slider";
import { useInstance } from "@/fuwa/hooks";
import { DARK_THEMES, LIGHT_THEMES, setPrefs, usePrefs, type MessageDisplay } from "@/lib/prefs";
import { BUILTIN_THEMES } from "@/lib/themes";
import { cn } from "@/lib/utils";
import { PrefSetting, ThemeGrid } from "./common";

export function Appearance({ instanceKey }: { instanceKey?: string }) {
  const p = usePrefs((x) => x);
  return (
    <WithPreview preview={<ChatPreview instanceKey={instanceKey} />}>
      <div className="flex flex-col">
        <PrefSetting id="theme" title="Theme" hint="The colors of the whole app, from the waifu.dev themes." keys={["theme", "followSystem", "lightTheme", "darkTheme"]}>
          <Toggle
            checked={p.followSystem}
            onChange={(followSystem) => setPrefs({ followSystem })}
            label="Follow my system"
            hint="A light theme by day and a dark one by night, as your system switches."
          />
          <AnimatePresence mode="wait" initial={false}>
            {p.followSystem ? (
              <motion.div key="system" initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={SPRING} className="flex flex-col gap-4">
                <div>
                  <p className="mb-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">When your system is light</p>
                  <ThemeGrid id="light" themes={LIGHT_THEMES} value={p.lightTheme} onChange={(lightTheme) => setPrefs({ lightTheme })} />
                </div>
                <div>
                  <p className="mb-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">When it's dark</p>
                  <ThemeGrid id="dark" themes={DARK_THEMES} value={p.darkTheme} onChange={(darkTheme) => setPrefs({ darkTheme })} />
                </div>
              </motion.div>
            ) : (
              <motion.div key="fixed" initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={SPRING}>
                <ThemeGrid id="fixed" themes={BUILTIN_THEMES} value={p.theme} onChange={(theme) => setPrefs({ theme })} />
              </motion.div>
            )}
          </AnimatePresence>
        </PrefSetting>
        <PrefSetting id="density" title="Density" hint="How much room messages and lists get." keys={["density"]}>
          <Choice
            value={p.density}
            onChange={(density) => setPrefs({ density })}
            options={[
              { value: "compact", label: "Compact", hint: "More on screen.", icon: <Rows4Icon className="size-4" /> },
              { value: "default", label: "Default", hint: "Balanced.", icon: <Rows3Icon className="size-4" /> },
              { value: "spacious", label: "Spacious", hint: "Room to breathe.", icon: <Rows2Icon className="size-4" /> },
            ]}
          />
        </PrefSetting>
        <PrefSetting id="message-display" title="Message display" keys={["messageDisplay"]}>
          <Choice<MessageDisplay>
            value={p.messageDisplay}
            onChange={(messageDisplay) => setPrefs({ messageDisplay })}
            options={[
              { value: "cozy", label: "Cozy", hint: "Avatars, and names over each run of messages.", icon: <MessageSquareTextIcon className="size-4" /> },
              { value: "compact", label: "Compact", hint: "Time and name in front of every line.", icon: <AlignJustifyIcon className="size-4" /> },
            ]}
          />
        </PrefSetting>
        <PrefSetting id="chat-font-size" title="Message text size" keys={["chatFontSize"]}>
          <Slider
            label="Message text size"
            value={p.chatFontSize}
            min={12}
            max={20}
            format={(n) => `${n}px`}
            onChange={(chatFontSize) => setPrefs({ chatFontSize })}
            marks={[
              { value: 12, label: "12px" },
              { value: 15, label: "15px" },
              { value: 20, label: "20px" },
            ]}
          />
        </PrefSetting>
        <Zoom />
      </div>
    </WithPreview>
  );
}

/** The whole app scales under the pointer, so zoom applies when the slider is let go. */
function Zoom() {
  const zoom = usePrefs((p) => p.zoom);
  const [value, setValue] = useState(zoom);
  useEffect(() => setValue(zoom), [zoom]);
  return (
    <PrefSetting id="zoom" title="Zoom" hint="The size of everything, applied when you let go." keys={["zoom"]}>
      <Slider
        label="Zoom"
        value={value}
        min={80}
        max={150}
        step={10}
        format={(n) => `${n}%`}
        onChange={setValue}
        onCommit={(z) => setPrefs({ zoom: z })}
        marks={[
          { value: 80, label: "80%" },
          { value: 100, label: "100%" },
          { value: 150, label: "150%" },
        ]}
      />
    </PrefSetting>
  );
}

const ago = (minutes: number) => new Date(Date.now() - minutes * 60_000);

/** A few messages in the current look, so density, display and text size show before you leave. */
export function ChatPreview({ instanceKey }: { instanceKey?: string }) {
  const display = usePrefs((p) => p.messageDisplay);
  usePrefs((p) => p.clock);
  const me = useInstance(instanceKey)?.me;
  const hana = create(UserSchema, { id: "01HANA", username: "hana", displayName: "Hana" });
  const you = me ?? create(UserSchema, { id: "01YOU", username: "you", displayName: "You" });
  const lines = [
    { author: hana, first: true, at: ago(6), text: "Has anyone tried the new themes? 🌸" },
    { author: hana, first: false, at: ago(5), text: "Sora feels like a summer morning" },
    { author: you, first: true, at: ago(1), text: "Yes! **Matcha** is my favorite so far" },
  ];
  return (
    <div className="overflow-hidden rounded-3xl border bg-background py-3 shadow-lg">
      {lines.map((line, n) => (
        <motion.div
          key={n}
          layout="position"
          transition={SPRING}
          className={cn("message-row flex gap-3 px-4", line.first && "first", display === "compact" && "compact")}
        >
          <MessageLine display={display} first={line.first} author={line.author} member={undefined} date={line.at}>
            <MessageBody content={line.text} display={display} />
          </MessageLine>
        </motion.div>
      ))}
    </div>
  );
}

/** Motion follows the system unless someone picks otherwise. */
export function MotionChoice() {
  const value = usePrefs((p) => p.reduceMotion);
  return (
    <Choice
      value={value}
      onChange={(reduceMotion) => setPrefs({ reduceMotion })}
      options={[
        { value: "system", label: "Like my system", hint: "Calmer when your system asks for less motion.", icon: <MonitorIcon className="size-4" /> },
        { value: "always", label: "Reduce it", hint: "Fades instead of springs and slides.", icon: <SnailIcon className="size-4" /> },
        { value: "never", label: "Full motion", hint: "Every spring and sparkle.", icon: <SparklesIcon className="size-4" /> },
      ]}
    />
  );
}
