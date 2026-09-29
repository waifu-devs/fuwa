import { RouterProvider } from "@tanstack/react-router";
import { MotionConfig } from "motion/react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { refreshOnFocus } from "@/fuwa/actions";
import { restore } from "@/fuwa/sync";
import { watchUnread } from "@/lib/notify";
import { applyPrefs, usePrefs, watchSystem } from "@/lib/prefs";
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

function App() {
  // Springs and slides calm down to fades when the system asks for less motion, or the Motion setting says so.
  const reduce = usePrefs((p) => p.reduceMotion);
  return (
    <MotionConfig reducedMotion={reduce === "system" ? "user" : reduce}>
      <RouterProvider router={router} />
    </MotionConfig>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
