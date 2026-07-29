import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { IconChevronDown } from "./UiIcons";

export type SelectOption = {
  value: string;
  label: string;
  disabled?: boolean;
  leading?: ReactNode;
};

type Props = {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  label?: string;
  disabled?: boolean;
  placeholder?: string;
  className?: string;
  id?: string;
  /** Stretch trigger to fill row (settings controls). */
  compact?: boolean;
};

export function Select({
  value,
  options,
  onChange,
  label,
  disabled,
  placeholder = "Select…",
  className,
  id,
  compact,
}: Props) {
  const autoId = useId();
  const triggerId = id ?? autoId;
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLUListElement>(null);
  const [open, setOpen] = useState(false);
  const [menuStyle, setMenuStyle] = useState<CSSProperties>({});
  const selected = options.find((o) => o.value === value);

  function placeMenu() {
    const trigger = triggerRef.current;
    if (!trigger) return;
    const rect = trigger.getBoundingClientRect();
    const gap = 6;
    const edge = 8;
    const spaceBelow = Math.max(0, window.innerHeight - rect.bottom - gap - edge);
    const spaceAbove = Math.max(0, rect.top - gap - edge);
    const estimated = Math.min(280, Math.max(options.length, 1) * 44 + 16);
    // Prefer below when it fits; otherwise open upward into the larger space.
    const openUp = spaceBelow < estimated && spaceAbove > spaceBelow;
    const available = openUp ? spaceAbove : spaceBelow;
    const maxHeight = Math.min(280, Math.max(available, 1));
    const width = Math.max(rect.width, 200);
    const left = Math.min(Math.max(edge, rect.left), window.innerWidth - width - edge);

    if (openUp) {
      setMenuStyle({
        position: "fixed",
        left,
        width,
        top: "auto",
        bottom: window.innerHeight - rect.top + gap,
        maxHeight,
        zIndex: 4000,
      });
    } else {
      setMenuStyle({
        position: "fixed",
        left,
        width,
        top: rect.bottom + gap,
        bottom: "auto",
        maxHeight,
        zIndex: 4000,
      });
    }
  }

  useLayoutEffect(() => {
    if (!open) return;
    placeMenu();
  }, [open, options.length]);

  useEffect(() => {
    if (!open) return;
    function onDoc(e: MouseEvent) {
      const t = e.target as Node;
      if (rootRef.current?.contains(t) || menuRef.current?.contains(t)) return;
      setOpen(false);
    }
    function onKey(e: globalThis.KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    function onReposition() {
      placeMenu();
    }
    document.addEventListener("mousedown", onDoc);
    document.addEventListener("keydown", onKey);
    window.addEventListener("resize", onReposition);
    window.addEventListener("scroll", onReposition, true);
    return () => {
      document.removeEventListener("mousedown", onDoc);
      document.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", onReposition);
      window.removeEventListener("scroll", onReposition, true);
    };
  }, [open, options.length]);

  function onTriggerKey(e: KeyboardEvent<HTMLButtonElement>) {
    if (disabled) return;
    if (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      setOpen(true);
    }
  }

  const menu =
    open && typeof document !== "undefined"
      ? createPortal(
          <ul
            ref={menuRef}
            className="select__menu select__menu--portal"
            role="listbox"
            aria-labelledby={triggerId}
            style={menuStyle}
          >
            {options.map((opt) => {
              const active = opt.value === value;
              return (
                <li key={opt.value} role="presentation">
                  <button
                    type="button"
                    role="option"
                    aria-selected={active}
                    disabled={opt.disabled}
                    className={`select__option${active ? " is-active" : ""}`}
                    onClick={() => {
                      if (opt.disabled) return;
                      onChange(opt.value);
                      setOpen(false);
                    }}
                  >
                    {opt.leading ? (
                      <span className="select__leading">{opt.leading}</span>
                    ) : null}
                    <span>{opt.label}</span>
                  </button>
                </li>
              );
            })}
          </ul>,
          document.body,
        )
      : null;

  return (
    <div
      className={[
        "field select-field",
        compact ? "select-field--compact" : "",
        className ?? "",
      ]
        .filter(Boolean)
        .join(" ")}
      ref={rootRef}
    >
      {label ? <label htmlFor={triggerId}>{label}</label> : null}
      <div className={`select${open ? " is-open" : ""}${disabled ? " is-disabled" : ""}`}>
        <button
          ref={triggerRef}
          id={triggerId}
          type="button"
          className="select__trigger"
          disabled={disabled}
          aria-haspopup="listbox"
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
          onKeyDown={onTriggerKey}
        >
          <span className="select__value">
            {selected?.leading ? (
              <span className="select__leading">{selected.leading}</span>
            ) : null}
            <span className={selected ? "" : "is-placeholder"}>
              {selected?.label ?? placeholder}
            </span>
          </span>
          <IconChevronDown size={16} className="select__chevron" />
        </button>
      </div>
      {menu}
    </div>
  );
}
