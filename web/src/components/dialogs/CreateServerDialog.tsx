import { useNavigate } from "@tanstack/react-router";
import { LoaderCircleIcon, MapPinIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import { ServerCreation } from "@/gen/fuwa/v1/types_pb";
import { createServer } from "@/fuwa/actions";
import { useAction, useInstances } from "@/fuwa/hooks";
import { ServerIcon } from "@/components/Icons";
import { PictureField } from "@/components/PictureField";
import { Private } from "@/components/Private";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { hasRegions, regionMark } from "@/lib/regions";
import { useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

export function CreateServerDialog({
  open,
  onOpenChange,
  defaultInstance,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  defaultInstance?: string;
}) {
  const { t } = useI18n();
  const navigate = useNavigate();
  const instances = useInstances().filter((i) => i.me);
  const [where, setWhere] = useState(defaultInstance ?? instances[0]?.key ?? "");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [discoverable, setDiscoverable] = useState(false);
  const [iconUrl, setIconUrl] = useState("");
  const [region, setRegion] = useState("");
  const create = useAction(createServer);
  const inst = instances.find((i) => i.key === where) ?? instances[0];
  const policy = inst?.node?.serverCreation;
  const regions = inst?.node?.regions ?? [];
  // The home region unless another was picked on this fuwa server.
  const picked = regions.find((r) => r.id === region) ?? regions.find((r) => r.home) ?? regions[0];
  const blocked =
    policy === ServerCreation.DISABLED || (policy === ServerCreation.ADMINS && !inst?.admin)
      ? t("workspace.createServer.blocked")
      : null;

  useEffect(() => {
    if (open) {
      setName("");
      setDescription("");
      setDiscoverable(false);
      setIconUrl("");
      setRegion("");
      create.setError(null);
      if (defaultInstance && instances.some((i) => i.key === defaultInstance)) setWhere(defaultInstance);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // An uploaded icon lives on the fuwa server it went to.
  useEffect(() => {
    setIconUrl("");
    setRegion("");
  }, [inst?.key]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!inst) return;
    const server = await create.go(inst.key, name.trim(), description.trim(), discoverable, iconUrl, hasRegions(regions) && !picked?.home ? (picked?.id ?? "") : "");
    if (!server) return;
    onOpenChange(false);
    navigate({ to: "/$instance/$server", params: { instance: inst.key, server: server.id } });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader title={t("workspace.createServer.title")} description={t("workspace.createServer.about")} />
        <form onSubmit={submit} className="flex flex-col gap-4">
          <div className="flex items-center gap-4">
            {inst && (
              <PictureField
                compact
                id="server-icon"
                instanceKey={inst.key}
                kind="icon"
                value={iconUrl}
                onChange={setIconUrl}
                disabled={!!blocked}
                fallback={
                  <motion.span key={name.trim().slice(0, 1) || "empty"} initial={{ scale: 0.8, rotate: -8 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 500, damping: 15 }} className="block size-full">
                    <ServerIcon server={{ id: name || "new", name: name || "?", iconUrl: "" }} active className="size-full text-xl" />
                  </motion.span>
                }
              />
            )}
            <div className="flex min-w-0 flex-1 flex-col gap-2">
              <Label htmlFor="server-name" className="font-bold">
                {t("workspace.createServer.name")}
              </Label>
              <Input
                id="server-name"
                autoFocus
                required
                maxLength={100}
                placeholder={t("workspace.createServer.namePlaceholder")}
                value={name}
                onChange={(e) => setName(e.target.value)}
                className="h-11 rounded-xl"
              />
            </div>
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="server-description" className="font-bold">
              {t("workspace.createServer.description")} <span className="font-normal text-muted-foreground">{t("workspace.createServer.optional")}</span>
            </Label>
            <Textarea
              id="server-description"
              rows={2}
              maxLength={1000}
              placeholder={t("workspace.createServer.descriptionPlaceholder")}
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              className="rounded-xl"
            />
          </div>
          {instances.length > 1 && (
            <div className="flex flex-col gap-2">
              <Label className="font-bold">{t("workspace.createServer.livesOn")}</Label>
              <div className="flex flex-wrap gap-2">
                {instances.map((i) => (
                  <button
                    key={i.key}
                    type="button"
                    onClick={() => setWhere(i.key)}
                    className={cn(
                      "rounded-full border px-3 py-1.5 text-sm font-bold transition",
                      i.key === inst?.key ? "border-primary bg-primary/15 text-primary" : "hover:border-primary/50",
                    )}
                  >
                    {i.node?.name ?? <Private text={i.key} />}
                  </button>
                ))}
              </div>
            </div>
          )}
          <AnimatePresence initial={false}>
            {hasRegions(regions) && (
              <motion.div
                key={inst?.key}
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: "auto" }}
                exit={{ opacity: 0, height: 0 }}
                transition={{ type: "spring", stiffness: 420, damping: 34 }}
                className="flex flex-col gap-2 overflow-hidden"
              >
                <Label className="font-bold">{t("workspace.createServer.region")}</Label>
                <div role="radiogroup" aria-label={t("workspace.createServer.region")} className="flex flex-wrap gap-2">
                  {regions.map((r) => {
                    const on = r.id === picked?.id;
                    return (
                      <button
                        key={r.id}
                        type="button"
                        role="radio"
                        aria-checked={on}
                        onClick={() => setRegion(r.id)}
                        className={cn("relative flex items-center gap-2 rounded-full border py-1.5 pr-3 pl-1.5 text-sm font-bold transition-colors", on ? "border-primary text-primary" : "hover:border-primary/50")}
                      >
                        {on && <motion.span layoutId="create-server-region" transition={{ type: "spring", stiffness: 500, damping: 35 }} className="absolute inset-0 rounded-full bg-primary/15" />}
                        <span className={cn("relative grid size-6 place-items-center rounded-full text-[0.6rem] font-extrabold transition-colors", on ? "bg-primary text-primary-foreground" : "bg-muted text-muted-foreground")}>
                          {regionMark(r.name)}
                        </span>
                        <span className="relative">{r.name}</span>
                        {r.home && <span className="relative text-xs font-normal text-muted-foreground">{t("workspace.createServer.home")}</span>}
                      </button>
                    );
                  })}
                </div>
                <AnimatePresence mode="popLayout" initial={false}>
                  <motion.p
                    key={picked?.id}
                    initial={{ opacity: 0, y: 4 }}
                    animate={{ opacity: 1, y: 0 }}
                    exit={{ opacity: 0, y: -4 }}
                    transition={{ duration: 0.18 }}
                    className="flex items-start gap-1.5 text-xs text-muted-foreground"
                  >
                    <MapPinIcon className="mt-px size-3.5 shrink-0 text-primary" />
                    {t("workspace.createServer.regionNote", { region: picked?.name ?? "", home: regions.find((r) => r.home)?.name ?? t("workspace.createServer.homeRegion") })}
                  </motion.p>
                </AnimatePresence>
              </motion.div>
            )}
          </AnimatePresence>
          <label className="flex cursor-pointer items-center justify-between gap-4 rounded-2xl border p-3">
            <span>
              <span className="block text-sm font-bold">{t("workspace.createServer.browse")}</span>
              <span className="block text-xs text-muted-foreground">
                {discoverable ? t("workspace.createServer.browseOn", { instance: inst?.node?.name ?? t("workspace.createServer.thisInstance") }) : t("workspace.createServer.browseOff")}
              </span>
            </span>
            <Switch checked={discoverable} onCheckedChange={setDiscoverable} />
          </label>
          <AnimatePresence>
            {(create.error || blocked) && (
              <motion.p initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} className="text-sm text-destructive first-letter:uppercase">
                {blocked ?? create.error}
              </motion.p>
            )}
          </AnimatePresence>
          <Button type="submit" size="lg" disabled={create.pending || !name.trim() || !!blocked || !inst} className="btn h-11 rounded-xl font-bold">
            {create.pending && <LoaderCircleIcon className="animate-spin" />}
            {t("workspace.createServer.submit")}
          </Button>
        </form>
      </DialogContent>
    </Dialog>
  );
}
