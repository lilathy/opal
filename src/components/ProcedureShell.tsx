import type { ReactNode } from "react";

type Step = { id: string; label: string };

type Props = {
  steps: Step[];
  activeId: string;
  children: ReactNode;
  direction?: "forward" | "back";
  ariaLabel?: string;
};

/** Linear procedure chrome — same pattern as Add Portfolio. */
export function ProcedureShell({
  steps,
  activeId,
  children,
  direction = "forward",
  ariaLabel,
}: Props) {
  const currentIndex = Math.max(
    0,
    steps.findIndex((s) => s.id === activeId),
  );

  return (
    <div className="procedure">
      <div className="wizard-steps" role="list" aria-label={ariaLabel}>
        {steps.map((s, i) => {
          const done = i < currentIndex;
          const active = s.id === activeId;
          return (
            <div
              key={s.id}
              role="listitem"
              className={`wizard-step${active ? " is-active" : ""}${done ? " is-done" : ""}`}
            >
              <span className="wizard-step__bar" aria-hidden />
              <span className="wizard-step__label">{s.label}</span>
            </div>
          );
        })}
      </div>
      <div className="wizard-layout">
        <div
          key={activeId}
          className={`wizard-pane${direction === "forward" ? " wizard-pane--fwd" : " wizard-pane--back"}`}
        >
          {children}
        </div>
      </div>
    </div>
  );
}
