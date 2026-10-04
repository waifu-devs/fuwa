import { CheckIcon, FolderIcon, FolderMinusIcon, FolderOpenIcon, FolderPlusIcon, PaletteIcon, Trash2Icon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState, type CSSProperties, type FormEvent, type ReactNode } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { ServerIcon } from "@/components/Icons";
import { Count } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { FOLDER_COLORS, FOLDER_NAME_MAX, folderHex, folderLabel, type RailFolder } from "@/lib/rail";
import { setFolderOpen, useFolderOpen } from "@/lib/rail-open";
import { cn } from "@/lib/utils";

const folderStyle = (folder: Pick<RailFolder, "color">) => ({ "--folder": folderHex(folder.color) ?? "var(--primary)" }) as CSSProperties;

/**
 * A folder on the rail: closed, a tile of its first servers' icons with their
 * unread messages added up; open, a folder icon with its servers under it on
 * a backdrop of its color. `children` are its servers, shown while open.
 */
export function FolderBlock({
  instance,
  folder,
  servers,
  unread,
  activeInside,
  onMenu,
  children,
}: {
  instance: string;
  folder: RailFolder;
  servers: Map<string, Server>;
  unread: number;
  /** Whether the server on screen is in it. */
  activeInside: boolean;
  onMenu: (e: React.MouseEvent) => void;
  children: ReactNode;
}) {
  const open = useFolderOpen(instance, folder.id);
  const [hover, setHover] = useState(false);
  const label = folderLabel(folder, servers);
  // Discord's left pill: shown for a closed folder holding the server on screen, or unread ones.
  const pill = !open && activeInside ? 40 : hover ? 20 : !open && unread > 0 ? 8 : 0;
  const tiles = folder.servers.slice(0, 4).flatMap((id) => {
    const s = servers.get(id);
    return s ? [s] : [];
  });
  return (
    <div data-rail-unit data-id={folder.id} className="relative flex w-full flex-col items-center gap-2" style={folderStyle(folder)}>
      <span data-rail-bg aria-hidden data-open={open} className="rail-folder-bg" />
      <div
        data-rail="folder"
        data-id={folder.id}
        data-folder=""
        data-open={open || undefined}
        className="relative flex w-full justify-center"
        onPointerEnter={(e) => e.pointerType === "mouse" && setHover(true)}
        onPointerLeave={() => setHover(false)}
      >
        <motion.span
          aria-hidden
          className="absolute top-1/2 left-0 -mt-5 h-10 w-1 rounded-r-full bg-foreground"
          initial={false}
          animate={{ scaleY: pill / 40, opacity: pill ? 1 : 0 }}
          transition={{ type: "spring", stiffness: 500, damping: 30 }}
        />
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              aria-expanded={open}
              aria-label={`${label}, folder of ${folder.servers.length}${unread ? `, ${unread} unread` : ""}`}
              onClick={() => setFolderOpen(instance, folder.id, !open)}
              onContextMenu={onMenu}
              className="relative rounded-[16px] outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <FolderTile open={open} tiles={tiles} />
              <AnimatePresence>
                {unread > 0 && !open && (
                  <motion.span
                    initial={{ scale: 0 }}
                    animate={{ scale: 1 }}
                    exit={{ scale: 0 }}
                    transition={{ type: "spring", stiffness: 600, damping: 18 }}
                    className="absolute -right-1 -bottom-1 grid h-5 min-w-5 place-items-center rounded-full bg-destructive px-1 text-[0.65rem] font-extrabold text-white ring-[3px] ring-[color-mix(in_srgb,var(--background)_75%,black)]"
                  >
                    <Count value={unread} max={99} />
                  </motion.span>
                )}
              </AnimatePresence>
            </button>
          </TooltipTrigger>
          <TooltipContent side="right" className="font-bold">
            {label}
          </TooltipContent>
        </Tooltip>
      </div>
      <AnimatePresence initial={false} mode="popLayout">
        {open && children}
      </AnimatePresence>
    </div>
  );
}

/** The folder's face: its servers in a 2×2 tile while closed, a folder icon while open. */
function FolderTile({ open, tiles }: { open: boolean; tiles: Pick<Server, "id" | "name" | "iconUrl">[] }) {
  return (
    <span className="rail-folder" data-open={open}>
      <span className="rail-folder-tiles" aria-hidden>
        {tiles.map((s) => (
          <ServerIcon key={s.id} server={s} className="rail-folder-mini" />
        ))}
      </span>
      <FolderOpenIcon aria-hidden className="rail-folder-glyph" />
    </span>
  );
}

/** What a rail menu is for. */
export type RailMenuFor = { kind: "folder"; id: string } | { kind: "server"; id: string; folder: string };
export type RailMenuTarget = RailMenuFor & { x: number; y: number };

/**
 * The rail's right-click menu for folders and servers, at the pointer.
 * (The app-wide menu component can take these items over once it lands.)
 */
export function RailMenu({
  target,
  open,
  onClose,
  onToggle,
  onEdit,
  onDissolve,
  onNewFolder,
  onLeaveFolder,
}: {
  target: RailMenuTarget | null;
  open: boolean;
  onClose: () => void;
  onToggle: (folder: string) => void;
  onEdit: (folder: string) => void;
  onDissolve: (folder: string) => void;
  onNewFolder: (server: string) => void;
  onLeaveFolder: (server: string) => void;
}) {
  return (
    <DropdownMenu open={open && !!target} onOpenChange={(o) => !o && onClose()} modal={false}>
      <DropdownMenuTrigger asChild>
        <span aria-hidden className="pointer-events-none fixed size-px" style={{ left: target?.x ?? 0, top: target?.y ?? 0 }} />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="right" align="start" className="w-56">
        {target?.kind === "folder" && (
          <>
            <DropdownMenuItem onSelect={() => onToggle(target.id)}>
              <FolderIcon /> Open or close
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={() => onEdit(target.id)}>
              <PaletteIcon /> Rename and recolor
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" onSelect={() => onDissolve(target.id)}>
              <Trash2Icon /> Dissolve folder
            </DropdownMenuItem>
          </>
        )}
        {target?.kind === "server" &&
          (target.folder ? (
            <DropdownMenuItem onSelect={() => onLeaveFolder(target.id)}>
              <FolderMinusIcon /> Take out of folder
            </DropdownMenuItem>
          ) : (
            <DropdownMenuItem onSelect={() => onNewFolder(target.id)}>
              <FolderPlusIcon /> Put in a new folder
            </DropdownMenuItem>
          ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Renames and recolors a folder, with a preview of its tile. */
export function FolderDialog({
  folder,
  servers,
  onOpenChange,
  onSave,
}: {
  folder: RailFolder | null;
  servers: Map<string, Server>;
  onOpenChange: (open: boolean) => void;
  onSave: (name: string, color: number) => void;
}) {
  return (
    <Dialog open={!!folder} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm">
        <DialogHeader title="Folder" description="Only you see your folders, on every device you sign in on." />
        {folder && <FolderForm key={folder.id} folder={folder} servers={servers} onSave={onSave} />}
      </DialogContent>
    </Dialog>
  );
}

function FolderForm({ folder, servers, onSave }: { folder: RailFolder; servers: Map<string, Server>; onSave: (name: string, color: number) => void }) {
  const [name, setName] = useState(folder.name);
  const [color, setColor] = useState(folder.color);
  const tiles = folder.servers.slice(0, 4).flatMap((id) => {
    const s = servers.get(id);
    return s ? [s] : [];
  });
  function submit(e: FormEvent) {
    e.preventDefault();
    onSave(name.trim(), color);
  }
  return (
    <form onSubmit={submit} className="flex flex-col gap-5">
      <div className="flex items-center gap-4" style={folderStyle({ color })}>
        <span className="rail-folder-preview">
          <FolderTile open={false} tiles={tiles} />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
          <Label htmlFor="folder-name">Name</Label>
          <Input
            id="folder-name"
            value={name}
            maxLength={FOLDER_NAME_MAX}
            placeholder={folderLabel({ ...folder, name: "" }, servers)}
            onChange={(e) => setName(e.target.value)}
            autoFocus
          />
        </div>
      </div>
      <fieldset className="flex flex-col gap-2">
        <legend className="mb-2 text-sm font-bold">Color</legend>
        <div className="flex flex-wrap gap-2">
          {FOLDER_COLORS.map((c) => (
            <button
              key={c}
              type="button"
              aria-pressed={color === c}
              aria-label={c ? folderHex(c)! : "Theme accent"}
              onClick={() => setColor(c)}
              style={folderStyle({ color: c })}
              className={cn(
                "rail-swatch grid size-8 place-items-center rounded-full text-white outline-none focus-visible:ring-2 focus-visible:ring-ring",
                color === c && "on",
              )}
            >
              <CheckIcon className="size-4" />
            </button>
          ))}
        </div>
      </fieldset>
      <Button type="submit">Save</Button>
    </form>
  );
}
