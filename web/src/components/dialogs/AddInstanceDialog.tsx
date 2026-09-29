import { useNavigate } from "@tanstack/react-router";
import { Connect } from "@/components/Connect";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";

export function AddInstanceDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const navigate = useNavigate();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader
          title="Connect to a fuwa server"
          description="Hosted or self-hosted: every fuwa server you add shows up in your rail, side by side."
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
