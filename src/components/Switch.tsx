import { useId } from "react";

type Props = {
  checked: boolean;
  onChange: (next: boolean) => void;
  disabled?: boolean;
  label: string;
  hint?: string;
  id?: string;
};

export function Switch({ checked, onChange, disabled, label, hint, id }: Props) {
  const autoId = useId();
  const switchId = id ?? autoId;

  return (
    <label
      className={`switch-row${disabled ? " is-disabled" : ""}`}
      htmlFor={switchId}
    >
      <span className="switch-row__copy">
        <span className="switch-row__title">{label}</span>
        {hint ? <span className="switch-row__hint">{hint}</span> : null}
      </span>
      <button
        id={switchId}
        type="button"
        role="switch"
        aria-checked={checked}
        disabled={disabled}
        className={`switch${checked ? " is-on" : ""}`}
        onClick={() => onChange(!checked)}
      >
        <span className="switch__thumb" />
      </button>
    </label>
  );
}
