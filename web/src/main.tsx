import { RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { restore } from "@/fuwa/sync";
import { applyTheme, savedTheme } from "@/lib/themes";
import { router } from "@/router";
import "@/styles/app.css";

applyTheme(savedTheme(), false);
restore();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <RouterProvider router={router} />
  </StrictMode>,
);
