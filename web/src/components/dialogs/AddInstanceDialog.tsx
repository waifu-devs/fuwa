import { useNavigate } from "@tanstack/react-router";
import { Connect } from "@/components/Connect";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { useI18n } from "@/i18n/react";

export function AddInstanceDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { t } = useI18n();
  const navigate = useNavigate();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader
          title={t("workspace.addInstance.title")}
          description={t("workspace.addInstance.about")}
        />
        <Connect
          onDone={(key) => {
            onOpenChange(false);
            navigate({ to: "/$instance", params: { instance: key } });
          }}
        />
      </DialogContent>
    </Dialog>
  );
}
