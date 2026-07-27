import { useId, useState, type InputHTMLAttributes } from "react";
import { useTranslation } from "react-i18next";
import { IconEye, IconEyeOff } from "./UiIcons";

type Props = Omit<InputHTMLAttributes<HTMLInputElement>, "type"> & {
  label?: string;
};

export function PasswordInput({ label, id, className, disabled, ...rest }: Props) {
  const { t } = useTranslation();
  const autoId = useId();
  const inputId = id ?? autoId;
  const [show, setShow] = useState(false);

  return (
    <div className={className ? `field password-field ${className}` : "field password-field"}>
      {label ? <label htmlFor={inputId}>{label}</label> : null}
      <div className="password-shell">
        <input
          {...rest}
          id={inputId}
          type={show ? "text" : "password"}
          disabled={disabled}
          className="password-shell__input"
        />
        <button
          type="button"
          className="password-shell__toggle"
          onClick={() => setShow((v) => !v)}
          disabled={disabled}
          aria-label={show ? t("common.hide") : t("common.show")}
          tabIndex={-1}
        >
          {show ? <IconEyeOff size={18} /> : <IconEye size={18} />}
        </button>
      </div>
    </div>
  );
}
