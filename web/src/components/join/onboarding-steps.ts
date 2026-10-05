import { OnboardingStepKind, type Onboarding, type OnboardingStep } from "@/gen/fuwa/v1/types_pb";
import { i18n } from "@/i18n/i18n";

/**
 * The steps someone goes through, as they'll see them: rules only while they
 * still have to agree (added when the admin left them out), hello only where
 * they can see the channel.
 */
export function stepsFor(onboarding: Pick<Onboarding, "steps">, opts: { mustAgree: boolean; canSeeChannel: (id: string) => boolean }): OnboardingStep[] {
  const steps = onboarding.steps.filter((s) => {
    if (s.kind === OnboardingStepKind.RULES) return opts.mustAgree;
    if (s.kind === OnboardingStepKind.HELLO) return opts.canSeeChannel(s.channelId);
    return s.kind === OnboardingStepKind.PICK && s.options.length > 0;
  });
  if (opts.mustAgree && !steps.some((s) => s.kind === OnboardingStepKind.RULES)) {
    const hello = steps.findIndex((s) => s.kind === OnboardingStepKind.HELLO);
    const rules = { kind: OnboardingStepKind.RULES, id: "rules", title: i18n().t("join.onboarding.rulesTitle"), description: "", skippable: false } as OnboardingStep;
    steps.splice(hello === -1 ? steps.length : hello, 0, rules);
  }
  return steps;
}
