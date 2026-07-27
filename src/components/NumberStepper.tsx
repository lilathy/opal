import { IconMinus, IconPlus } from "./UiIcons";

type Props = {
  value: number;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  /** Fires immediately on +/- clicks and once on blur after free-typing. */
  onChange: (value: number) => void;
  className?: string;
  "aria-label"?: string;
};

/** Compact +/- number control that replaces the browser's native `<input
 * type="number">` spinner arrows — those render with the OS's own theme
 * (a tiny grey up/down stack) that clashes with the rest of the app's
 * flat, dark, custom-drawn controls. */
export function NumberStepper({
  value,
  min = 0,
  max = Number.MAX_SAFE_INTEGER,
  step = 1,
  disabled,
  onChange,
  className,
  "aria-label": ariaLabel,
}: Props) {
  const clamp = (n: number) => Math.min(max, Math.max(min, n));

  return (
    <div className={`number-stepper${className ? ` ${className}` : ""}`}>
      <button
        type="button"
        className="number-stepper__btn"
        disabled={disabled || value <= min}
        onClick={() => onChange(clamp(value - step))}
        aria-label={ariaLabel ? `Decrease ${ariaLabel}` : "Decrease"}
      >
        <IconMinus size={13} />
      </button>
      <input
        className="number-stepper__input"
        type="text"
        inputMode="numeric"
        pattern="[0-9]*"
        value={value}
        disabled={disabled}
        aria-label={ariaLabel}
        onChange={(e) => {
          const digits = e.target.value.replace(/[^0-9]/g, "");
          if (digits === "") return;
          onChange(clamp(Number(digits)));
        }}
        onBlur={(e) => {
          if (e.target.value.trim() === "") onChange(clamp(min));
        }}
      />
      <button
        type="button"
        className="number-stepper__btn"
        disabled={disabled || value >= max}
        onClick={() => onChange(clamp(value + step))}
        aria-label={ariaLabel ? `Increase ${ariaLabel}` : "Increase"}
      >
        <IconPlus size={13} />
      </button>
    </div>
  );
}
