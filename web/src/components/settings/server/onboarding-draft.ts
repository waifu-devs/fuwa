import { create, equals } from "@bufbuild/protobuf";
import { OnboardingOptionSchema, OnboardingSchema, OnboardingStepSchema, type Onboarding, type OnboardingStep } from "@/gen/fuwa/v1/types_pb";

/** A step being edited, with a key that survives reordering. */
export type Draft = { key: string; step: OnboardingStep };

/** The onboarding as it's being edited. */
export type OnboardingDraft = { enabled: boolean; steps: Draft[] };

let made = 0;
export const fresh = () => `new-${++made}`;

export const onboardingDraft = (o: Onboarding): OnboardingDraft => ({
  enabled: o.enabled,
  steps: o.steps.map((step) => ({ key: step.id || fresh(), step })),
});

/** The draft as the server takes it. */
export const onboardingOf = (d: OnboardingDraft): Onboarding =>
  create(OnboardingSchema, {
    enabled: d.enabled,
    steps: d.steps.map(({ step }) =>
      create(OnboardingStepSchema, {
        ...step,
        title: step.title.trim(),
        description: step.description.trim(),
        hello: step.hello.trim(),
        options: step.options.map((o) => create(OnboardingOptionSchema, { ...o, label: o.label.trim(), description: o.description.trim() })),
      }),
    ),
  });

/** 1 when the onboarding differs from what's saved. */
export const onboardingChanges = (d: OnboardingDraft, saved: Onboarding) => (equals(OnboardingSchema, onboardingOf(d), saved) ? 0 : 1);
