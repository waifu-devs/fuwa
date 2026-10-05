import type { Node } from "@/gen/fuwa/v1/types_pb";
import { useI18n } from "@/i18n/react";

/**
 * "fuwa 0.4.2 · a0b78b7": the build an instance says it runs. The instance
 * reports this about itself, so it's plain text, never a sign of a verified
 * server. Only a hardware attestation checked by a client the instance didn't
 * serve can show that.
 */
export function BuildLabel({ node }: { node?: Node | null }) {
  const { t } = useI18n();
  if (!node) return null;
  const commit = node.build?.commit.slice(0, 7);
  return (
    <span>
      fuwa {node.version}
      {commit ? <span className="font-mono" title={t("shell.build.commit", { commit: node.build?.commit ?? "" })}> · {commit}</span> : null}
    </span>
  );
}
