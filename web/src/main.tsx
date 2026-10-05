import { RouterProvider } from "@tanstack/react-router";
import { LazyMotion, MotionConfig } from "motion/react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppBackdrop } from "@/components/Backdrop";
import { refreshOnFocus } from "@/fuwa/actions";
import { reportTarget, restore } from "@/fuwa/sync";
import { startReports } from "@/lib/reports";
import { watchUnread } from "@/lib/notify";
import { switchLanguage } from "@/i18n/i18n";
import { applyPrefs, getPrefs, subscribePrefs, usePrefs, watchSystem } from "@/lib/prefs";
import { router } from "@/router";
import "@fontsource/m-plus-rounded-1c/400.css";
import "@fontsource/m-plus-rounded-1c/700.css";
import "@fontsource/m-plus-rounded-1c/800.css";
import "@/styles/app.css";

applyPrefs();
watchSystem();
restore();
watchUnread();
refreshOnFocus();
startReports(reportTarget);

// The animation features download beside the language's strings; the first paint waits for both, so nothing shows unanimated.
const motionFeatures = import("@/lib/motion-features").then((m) => m.default);
const loadMotion = () => motionFeatures;

function App() {
  // Springs and slides calm down to fades when the system asks for less motion, or the Motion setting says so.
  const reduce = usePrefs((p) => p.reduceMotion);
  return (
    <LazyMotion features={loadMotion}>
      <MotionConfig reducedMotion={reduce === "system" ? "user" : reduce}>
        <RouterProvider router={router} />
        <AppBackdrop />
      </MotionConfig>
    </LazyMotion>
  );
}

// The Language setting, or the browser's languages while it says "auto".
subscribePrefs(() => void switchLanguage(getPrefs().language));
window.addEventListener("languagechange", () => void switchLanguage(getPrefs().language));

// The first paint waits for the language's strings (a small same-origin file), so the app doesn't flash in English.
void Promise.allSettled([switchLanguage(getPrefs().language), motionFeatures]).finally(() =>
  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  ),
);
