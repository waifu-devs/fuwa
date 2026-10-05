import { create } from "@bufbuild/protobuf";
import { useNavigate } from "@tanstack/react-router";
import { ArrowRightIcon, CheckIcon, UserPlusIcon } from "lucide-react";
import { Account } from "@/components/Connect";
import { UserAvatar } from "@/components/Icons";
import { Private } from "@/components/Private";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator } from "@/components/ui/dropdown-menu";
import { ownPicture, type SavedAccount } from "@/fuwa/saved";
import { useFuwa } from "@/fuwa/store";
import { keptAccounts, switchAccount } from "@/fuwa/sync";
import { UserSchema, type User } from "@/gen/fuwa/v1/types_pb";
import { useI18n } from "@/i18n/react";
import { displayName } from "@/lib/format";

/** An account kept here but not connected, drawn from the card saved with it. */
const userOf = (a: SavedAccount, url: string): User =>
  create(UserSchema, { id: a.userId, username: a.username, displayName: a.displayName, avatarUrl: ownPicture(a.avatarUrl, url) });

/**
 * The accounts kept on an instance, in the profile menu: the one in use with
 * a check, the others to switch to, then "Add an account". Switching sends
 * the shell to the instance's home, since the page open may be a server the
 * other account isn't in.
 */
export function AccountItems({ instanceKey, onAdd }: { instanceKey: string; onAdd: () => void }) {
  const { t } = useI18n();
  const meId = useFuwa((s) => s.instances[instanceKey]?.me?.id ?? "");
  const url = useFuwa((s) => s.instances[instanceKey]?.url ?? "");
  const accounts = keptAccounts(instanceKey).filter((a) => a.userId);
  return (
    <>
      <DropdownMenuSeparator />
      <DropdownMenuLabel className="text-xs text-muted-foreground">{t("connect.accounts.title")}</DropdownMenuLabel>
      {accounts.map((a) => {
        const active = a.userId === meId;
        const user = userOf(a, url);
        return (
          <DropdownMenuItem
            key={a.userId}
            onSelect={() => !active && switchAccount(instanceKey, a.userId)}
            aria-label={active ? undefined : t("connect.accounts.switchTo", { name: displayName(user) })}
            className="group gap-2.5 py-1.5"
          >
            <UserAvatar user={user} className="size-7 text-xs transition-transform duration-300 group-hover:scale-110" />
            <span className="min-w-0 flex-1">
              <span className="block truncate font-bold">{displayName(user)}</span>
              <span className="block truncate text-xs text-muted-foreground">
                @<Private text={a.username} kind="name" />
              </span>
            </span>
            {active && <CheckIcon className="text-primary" aria-label={t("connect.accounts.inUse")} />}
          </DropdownMenuItem>
        );
      })}
      <DropdownMenuItem onSelect={onAdd}>
        <UserPlusIcon /> {t("connect.accounts.add")}
      </DropdownMenuItem>
    </>
  );
}

/** Signs in to one more account on this instance; it becomes the one in use, and the others stay kept. */
export function AddAccountDialog({ instanceKey, open, onOpenChange }: { instanceKey: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const { t } = useI18n();
  const navigate = useNavigate();
  const inst = useFuwa((s) => s.instances[instanceKey]);
  if (!inst?.node) return null;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader
          title={t("connect.accounts.addTitle")}
          description={t("connect.accounts.addAbout", { instance: inst.node.name || instanceKey })}
        />
        <Account
          url={inst.url}
          node={inst.node}
          returnTo={`/${instanceKey}`}
          onDone={(key) => {
            onOpenChange(false);
            void navigate({ to: "/$instance", params: { instance: key } });
          }}
        />
      </DialogContent>
    </Dialog>
  );
}

/** On an instance's sign-in page: the other accounts kept here, to carry on as one of them without signing in again. */
export function ContinueAs({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const url = useFuwa((s) => s.instances[instanceKey]?.url ?? "");
  const accounts = keptAccounts(instanceKey).filter((a) => a.userId);
  if (!accounts.length) return null;
  return (
    <div className="mb-6 flex flex-col gap-2">
      <p className="text-sm font-bold">{t("connect.accounts.continueAs")}</p>
      <div className="stagger flex flex-col gap-2">
        {accounts.map((a) => {
          const user = userOf(a, url);
          return (
            <button
              key={a.userId}
              type="button"
              onClick={() => switchAccount(instanceKey, a.userId)}
              aria-label={t("connect.accounts.switchTo", { name: displayName(user) })}
              className="group card-pop flex items-center gap-3 rounded-2xl border bg-background/60 p-3 text-left"
            >
              <UserAvatar user={user} className="size-10 transition-transform duration-300 group-hover:-rotate-6 group-hover:scale-110" />
              <span className="min-w-0 flex-1">
                <span className="block truncate font-bold">{displayName(user)}</span>
                <span className="block truncate text-xs text-muted-foreground">
                  @<Private text={a.username} kind="name" />
                </span>
              </span>
              <ArrowRightIcon className="size-4 text-muted-foreground transition group-hover:translate-x-1 group-hover:text-primary" />
            </button>
          );
        })}
      </div>
      <p className="mt-2 text-center text-xs text-muted-foreground">{t("connect.accounts.orAnother")}</p>
    </div>
  );
}
