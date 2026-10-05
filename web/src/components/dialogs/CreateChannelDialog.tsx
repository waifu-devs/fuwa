import { useNavigate } from "@tanstack/react-router";
import { FolderIcon, HashIcon, LoaderCircleIcon, MegaphoneIcon, ShieldCheckIcon, Volume2Icon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import { ChannelType, createChannel } from "@/fuwa/actions";
import { useAction } from "@/fuwa/hooks";
import { SwapText } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

const TYPES = [
  { type: ChannelType.TEXT, icon: HashIcon, label: "workspace.createChannel.type.text", hint: "workspace.createChannel.type.textHint" },
  { type: ChannelType.ANNOUNCEMENT, icon: MegaphoneIcon, label: "workspace.createChannel.type.announcement", hint: "workspace.createChannel.type.announcementHint" },
  { type: ChannelType.SECURE, icon: ShieldCheckIcon, label: "workspace.createChannel.type.secure", hint: "workspace.createChannel.type.secureHint" },
  { type: ChannelType.VOICE, icon: Volume2Icon, label: "workspace.createChannel.type.voice", hint: "workspace.createChannel.type.voiceHint" },
  { type: ChannelType.CATEGORY, icon: FolderIcon, label: "workspace.createChannel.type.category", hint: "workspace.createChannel.type.categoryHint" },
] as const;

/** Channel names are lowercase with dashes, like the server makes them. */
const slug = (name: string) =>
  name
    .toLowerCase()
    .replace(/\s+/g, "-")
    .replace(/[^\p{L}\p{N}_-]/gu, "")
    .slice(0, 100);

export function CreateChannelDialog({
  open,
  onOpenChange,
  instanceKey,
  serverId,
  parentId = "",
  stay = false,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  serverId: string;
  parentId?: string;
  /** Stay where you are, rather than opening the new channel (as from settings). */
  stay?: boolean;
}) {
  const { t } = useI18n();
  const navigate = useNavigate();
  const [type, setType] = useState<ChannelType>(ChannelType.TEXT);
  const [name, setName] = useState("");
  const create = useAction(createChannel);
  const category = type === ChannelType.CATEGORY;
  /** Named as typed, like categories: "Lounge", not "#lounge". */
  const free = category || type === ChannelType.VOICE;

  useEffect(() => {
    if (open) {
      setName("");
      setType(ChannelType.TEXT);
      create.setError(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    const channel = await create.go(instanceKey, serverId, free ? name.trim() : slug(name), type, category ? "" : parentId);
    if (!channel) return;
    onOpenChange(false);
    if (!category && !stay) navigate({ to: "/$instance/$server/$channel", params: { instance: instanceKey, server: serverId, channel: channel.id } });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader title={t("workspace.createChannel.title")} />
        <form onSubmit={submit} className="flex flex-col gap-4">
          <div role="radiogroup" aria-label={t("workspace.createChannel.typeLabel")} className="grid gap-2">
            {TYPES.map((kind) => {
              const active = type === kind.type;
              return (
                <motion.button
                  key={kind.type}
                  type="button"
                  role="radio"
                  aria-checked={active}
                  onClick={() => setType(kind.type)}
                  whileHover={{ x: 2 }}
                  whileTap={{ scale: 0.98 }}
                  transition={SPRING}
                  className={cn(
                    "relative flex items-center gap-3 rounded-2xl border p-3 text-left transition-colors",
                    active ? "border-primary/60" : "hover:border-primary/40",
                  )}
                >
                  {active && (
                    <motion.span layoutId="channel-type" transition={SPRING} className="absolute inset-0 rounded-2xl bg-primary/10 ring-2 ring-primary/40" />
                  )}
                  <span
                    className={cn(
                      "relative grid size-9 shrink-0 place-items-center rounded-xl transition-colors",
                      active ? "bg-primary text-primary-foreground" : "bg-muted text-muted-foreground",
                    )}
                  >
                    <motion.span key={String(active)} initial={active ? { scale: 0.4, rotate: -30 } : false} animate={{ scale: 1, rotate: 0 }} transition={SPRING}>
                      <kind.icon className="size-[18px]" />
                    </motion.span>
                  </span>
                  <span className="relative">
                    <span className="block text-sm font-bold">{t(kind.label)}</span>
                    <span className="block text-xs text-muted-foreground">{t(kind.hint)}</span>
                  </span>
                </motion.button>
              );
            })}
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="channel-name" className="font-bold">
              {t("workspace.createChannel.name")}
            </Label>
            <div className="relative">
              <AnimatePresence initial={false}>
                {!category && (
                  <motion.span
                    key={type === ChannelType.VOICE ? "voice" : type === ChannelType.SECURE ? "secure" : "text"}
                    initial={{ opacity: 0, scale: 0.4, rotate: -30 }}
                    animate={{ opacity: 1, scale: 1, rotate: 0 }}
                    exit={{ opacity: 0, scale: 0.4 }}
                    transition={SPRING}
                    className="pointer-events-none absolute top-3.5 left-3 text-muted-foreground"
                  >
                    {type === ChannelType.VOICE ? (
                      <Volume2Icon className="size-4" />
                    ) : type === ChannelType.SECURE ? (
                      <ShieldCheckIcon className="size-4" />
                    ) : (
                      <HashIcon className="size-4" />
                    )}
                  </motion.span>
                )}
              </AnimatePresence>
              <Input
                id="channel-name"
                autoFocus
                required
                maxLength={100}
                placeholder={t(category ? "workspace.createChannel.placeholder.category" : type === ChannelType.VOICE ? "workspace.createChannel.placeholder.voice" : "workspace.createChannel.placeholder.text")}
                value={free ? name : slug(name)}
                onChange={(e) => setName(e.target.value)}
                className={cn("h-11 rounded-xl transition-[padding]", !category && "pl-9")}
              />
            </div>
          </div>
          {create.error && <p className="text-sm text-destructive first-letter:uppercase">{create.error}</p>}
          <Button type="submit" size="lg" disabled={create.pending || !name.trim()} className="btn h-11 rounded-xl font-bold">
            {create.pending && <LoaderCircleIcon className="animate-spin" />}
            <span>
              <SwapText>{t(category ? "workspace.createChannel.createCategory" : "workspace.createChannel.createChannel")}</SwapText>
            </span>
          </Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}
