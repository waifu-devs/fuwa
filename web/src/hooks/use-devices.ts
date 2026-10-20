import { useEffect, useState } from "react";

/** A device; `label` is empty until the browser has been allowed to name it, so it goes by its place in the list. */
export type Device = { id: string; label: string; n: number };

/** The microphones, speakers and cameras this browser can use. Names show once it's been allowed them. */
export function useDevices() {
  const [devices, setDevices] = useState<{ inputs: Device[]; outputs: Device[]; cameras: Device[] }>({ inputs: [], outputs: [], cameras: [] });
  useEffect(() => {
    if (!navigator.mediaDevices?.enumerateDevices) return;
    const read = () =>
      void navigator.mediaDevices.enumerateDevices().then((list) => {
        const pick = (kind: MediaDeviceKind) =>
          list
            .filter((d) => d.kind === kind && d.deviceId && d.deviceId !== "default" && d.deviceId !== "communications")
            .map((d, n) => ({ id: d.deviceId, label: d.label, n: n + 1 }));
        setDevices({ inputs: pick("audioinput"), outputs: pick("audiooutput"), cameras: pick("videoinput") });
      });
    read();
    navigator.mediaDevices.addEventListener("devicechange", read);
    return () => navigator.mediaDevices.removeEventListener("devicechange", read);
  }, []);
  return devices;
}
