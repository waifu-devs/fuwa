import { actionById, bindingOf, comboOf, normalize } from "@/lib/keybinds";
import { getPrefs } from "@/lib/prefs";
import { setPushing } from "./engine";

/**
 * Push to talk: talking while its key is held, anywhere in the app (even
 * in a text box: it's a key you picked for it). The key's last part going
 * up lets go, whichever modifier came up first; so does leaving the window.
 */
export function watchPushToTalk(): () => void {
  const binding = () => {
    const combo = bindingOf(actionById("pushToTalk")!, getPrefs());
    return getPrefs().inputMode === "ptt" && combo ? normalize(combo) : null;
  };
  let held: string | null = null;
  const down = (e: KeyboardEvent) => {
    const combo = binding();
    if (!combo || e.isComposing) return;
    const pressed = comboOf(e);
    if (!pressed || normalize(pressed) !== combo) return;
    e.preventDefault();
    held = e.code;
    setPushing(true);
  };
  const up = (e: KeyboardEvent) => {
    if (held && (e.code === held || ["Shift", "Control", "Alt", "Meta"].includes(e.key))) {
      held = null;
      setPushing(false);
    }
  };
  const blur = () => {
    held = null;
    setPushing(false);
  };
  window.addEventListener("keydown", down, true);
  window.addEventListener("keyup", up, true);
  window.addEventListener("blur", blur);
  return () => {
    window.removeEventListener("keydown", down, true);
    window.removeEventListener("keyup", up, true);
    window.removeEventListener("blur", blur);
  };
}
