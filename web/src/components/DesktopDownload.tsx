import { ChevronDownIcon, DownloadIcon, MonitorDownIcon } from "lucide-react";
import { m as motion } from "motion/react";
import { useRef, useState } from "react";
import type { DesktopDownload as Download, Node } from "@/gen/fuwa/v1/types_pb";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import type { Key } from "@/i18n/i18n";
import { useI18n } from "@/i18n/react";
import { downloadsFor, downloadUrl, PACKAGE, type System, systemOf } from "@/lib/desktop";
import { formatBytes } from "@/lib/format";
import { SPRING } from "@/lib/motion";

const SYSTEMS: System[] = ["windows", "macos", "linux"];
const NAMES: Record<System, string> = { windows: "Windows", macos: "macOS", linux: "Linux" };
const PACKAGES: Record<number, Key> = {
  [PACKAGE.setup]: "common.desktopApp.setup",
  [PACKAGE.dmg]: "common.desktopApp.dmg",
  [PACKAGE.appimage]: "common.desktopApp.appimage",
  [PACKAGE.deb]: "common.desktopApp.deb",
};

/**
 * "Download for Windows", with the other systems' files beside it: the
 * desktop app from the instance's latest release, passed through the
 * instance (`/updates/files/...`), so GitHub never sees who fetched it.
 * Nothing shows when the instance knows of no release, or on a phone.
 */
export function DesktopDownload({ node, url, align = "end" }: { node?: Node | null; url: string; align?: "start" | "end" }) {
  const lang = useI18n();
  const { t } = lang;
  // The menu hangs from the arrow; lined up on the left, it starts under the pair's first button.
  const button = useRef<HTMLAnchorElement>(null);
  const [offset, setOffset] = useState(0);
  const app = node?.desktopApp;
  const mine = typeof navigator === "undefined" ? null : systemOf(navigator.userAgent, navigator.maxTouchPoints);
  if (!app?.downloads.length || !mine) return null;
  const first = downloadsFor(app.downloads, mine)[0];
  const others = SYSTEMS.map((system) => [system, downloadsFor(app.downloads, system)] as const).filter(([, files]) => files.length > 0);
  const item = (system: System, file: Download) => (
    <DropdownMenuItem key={file.path} onSelect={() => save(downloadUrl(url, file.path))}>
      <DownloadIcon />
      <span className="flex-1">
        {NAMES[system]} · {t(PACKAGES[file.package] ?? "common.desktopApp.setup")}
      </span>
      <span className="text-xs text-muted-foreground">{formatBytes(lang, Number(file.size))}</span>
    </DropdownMenuItem>
  );
  return (
    <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex shrink-0">
      {first ? (
        <Button asChild size="lg" variant="outline" className="btn h-11 rounded-l-xl rounded-r-none border-r-0 font-bold">
          <a ref={button} href={downloadUrl(url, first.path)} download title={t("common.desktopApp.through")}>
            <MonitorDownIcon /> {t("common.desktopApp.download", { system: NAMES[mine] })}
          </a>
        </Button>
      ) : null}
      <DropdownMenu onOpenChange={(open) => open && setOffset(button.current?.offsetWidth ?? 0)}>
        <DropdownMenuTrigger asChild>
          <Button
            size="lg"
            variant="outline"
            aria-label={t("common.desktopApp.more")}
            className={first ? "btn h-11 rounded-l-none rounded-r-xl px-2.5" : "btn h-11 rounded-xl font-bold"}
          >
            {first ? null : (
              <>
                <MonitorDownIcon /> {t("common.desktopApp.get")}
              </>
            )}
            <ChevronDownIcon />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align={align} alignOffset={align === "start" ? -offset : 0} className="w-80">
          <DropdownMenuLabel className="text-xs text-muted-foreground">{t("common.desktopApp.version", { version: app.version })}</DropdownMenuLabel>
          {others.map(([system, files], n) => [n > 0 ? <DropdownMenuSeparator key={`${system}-line`} /> : null, ...files.map((file) => item(system, file))])}
          <DropdownMenuSeparator />
          <p className="px-2 py-1.5 text-xs text-muted-foreground">{t("common.desktopApp.through")}</p>
        </DropdownMenuContent>
      </DropdownMenu>
    </motion.div>
  );
}

/** Starts a download, as a link with `download` would. */
function save(href: string) {
  const link = document.createElement("a");
  link.href = href;
  link.download = "";
  link.click();
}
