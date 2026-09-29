import { RouterProvider } from "@tanstack/react-router";
import { MotionConfig } from "motion/react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { restore } from "@/fuwa/sync";
import { applyTheme, savedTheme } from "@/lib/themes";
import { router } from "@/router";
import "@fontsource/m-plus-rounded-1c/400.css";
import "@fontsource/m-plus-rounded-1c/700.css";
import "@fontsource/m-plus-rounded-1c/800.css";
import "@/styles/app.css";

applyTheme(savedTheme(), false);
restore();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {/* Springs and slides calm down to fades for people who ask their system for less motion. */}
    <MotionConfig reducedMotion="user">
      <RouterProvider router={router} />
    </MotionConfig>
  </StrictMode>,
);
