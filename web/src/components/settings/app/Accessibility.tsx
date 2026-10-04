import { CircleIcon, CircleOffIcon, PaletteIcon } from "lucide-react";
import { Markdown } from "@/components/Markdown";
import { RoleName } from "@/components/RoleName";
import { Choice, Toggle } from "@/components/settings/controls";
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
      <PrefSetting
        id="role-colors"
        title="Role colors"
        hint="Roles can give people's names a color. Show it on the name, as a dot beside it, or not at all."
        keys={["roleColors"]}
        delay={0.06}
      >
        <Choice
          value={p.roleColors}
          onChange={(roleColors) => setPrefs({ roleColors })}
          options={[
            { value: "names", label: "On names", hint: "Names take their role's color.", icon: <PaletteIcon className="size-4" /> },
            { value: "beside", label: "Beside names", hint: "A dot in the role's color.", icon: <CircleIcon className="size-4" /> },
            { value: "off", label: "Off", hint: "Names keep their own tint.", icon: <CircleOffIcon className="size-4" /> },
          ]}
        />
        <div className="flex flex-wrap gap-x-5 gap-y-1 rounded-xl bg-muted/50 px-3 py-2 text-sm">
          <RoleName id="preview-sakura" name="Sakura" color={0xf472b6} />
          <RoleName id="preview-ren" name="Ren" color={0x60a5fa} />
          <RoleName id="preview-mio" name="Mio" color={0x34d399} />
        </div>
      </PrefSetting>
      <PrefSetting
        id="others-effects"
        title="Profile effects"
        hint={reduceMotion(p) ? "With motion calmed, effects show as a still picture." : "Petals, stars and the like that people put on their profile cards."}
        keys={["othersEffects"]}
        delay={0.07}
      >
        <Toggle
          checked={p.othersEffects}
          onChange={(othersEffects) => setPrefs({ othersEffects })}
          label="Show effects on other people's cards"
          hint="Your own always shows to you, so you can see what others do."
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
