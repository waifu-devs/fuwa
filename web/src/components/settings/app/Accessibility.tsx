import { Markdown } from "@/components/Markdown";
import { Toggle } from "@/components/settings/controls";
import { Slider } from "@/components/ui/slider";
import { reduceMotion, setPrefs, usePrefs } from "@/lib/prefs";
import { MotionChoice } from "./Appearance";
import { PrefSetting } from "./common";

export function Accessibility() {
  const p = usePrefs((x) => x);
  const systemCalm = typeof window !== "undefined" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  return (
    <div className="flex flex-col">
      <PrefSetting
        id="reduce-motion"
        title="Motion"
        hint={
          p.reduceMotion === "system"
            ? systemCalm
              ? "Your system asks for less motion, so fuwa keeps it calm."
              : "Your system is fine with motion, so fuwa moves."
            : reduceMotion(p)
              ? "Springs, slides and sparkles are off."
              : "Everything moves, whatever your system says."
        }
        keys={["reduceMotion"]}
      >
        <MotionChoice />
      </PrefSetting>
      <PrefSetting id="saturation" title="Saturation" hint="Softens every color in the app, down to greys." keys={["saturation"]} delay={0.04}>
        <Slider
          label="Saturation"
          value={p.saturation}
          min={0}
          max={100}
          step={5}
          format={(n) => `${n}%`}
          onChange={(saturation) => setPrefs({ saturation })}
          marks={[
            { value: 0, label: "0%" },
            { value: 50, label: "50%" },
            { value: 100, label: "100%" },
          ]}
        />
      </PrefSetting>
      <PrefSetting id="underline-links" title="Links" keys={["underlineLinks"]} delay={0.08}>
        <Toggle
          checked={p.underlineLinks}
          onChange={(underlineLinks) => setPrefs({ underlineLinks })}
          label="Always underline links"
          hint="So links stand out by more than their color."
        />
        <div className="rounded-xl bg-muted/50 px-3 py-2 text-sm">
          <Markdown className="chat">{"The docs are at [fuwa's README](https://github.com/waifu-devs/fuwa) if you get stuck."}</Markdown>
        </div>
      </PrefSetting>
    </div>
  );
}
