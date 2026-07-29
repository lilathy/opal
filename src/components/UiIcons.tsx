import type { ReactNode } from "react";

type Props = {
  size?: number;
  className?: string;
};

function Ico({
  size = 18,
  className,
  children,
}: Props & { children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      className={className}
      aria-hidden
    >
      {children}
    </svg>
  );
}

export function IconSend(p: Props) {
  return (
    <Ico {...p}>
      <path
        d="M12 19V5M12 5l-6 6M12 5l6 6"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Ico>
  );
}

export function IconReceive(p: Props) {
  return (
    <Ico {...p}>
      <path
        d="M12 5v14M12 19l-6-6M12 19l6-6"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Ico>
  );
}

export function IconSwap(p: Props) {
  return (
    <Ico {...p}>
      <path
        d="M7 8h11M18 8l-3-3M18 8l-3 3M17 16H6M6 16l3-3M6 16l3 3"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Ico>
  );
}

/** Vertical flip - used on the swap panel divider. */
export function IconFlip(p: Props) {
  return (
    <Ico {...p}>
      <path
        d="M12 4v12M12 4l-3.5 3.5M12 4l3.5 3.5M12 20V8M12 20l-3.5-3.5M12 20l3.5-3.5"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Ico>
  );
}

export function IconSettings(p: Props) {
  return (
    <Ico {...p}>
      <circle cx="12" cy="12" r="3" stroke="currentColor" strokeWidth="1.8" />
      <path
        d="M12 3v2M12 19v2M3 12h2M19 12h2M5.6 5.6l1.4 1.4M17 17l1.4 1.4M5.6 18.4l1.4-1.4M17 7l1.4-1.4"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
      />
    </Ico>
  );
}

export function IconLock(p: Props) {
  return (
    <Ico {...p}>
      <rect
        x="5"
        y="11"
        width="14"
        height="10"
        rx="2"
        stroke="currentColor"
        strokeWidth="1.8"
      />
      <path
        d="M8 11V8a4 4 0 0 1 8 0v3"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
      />
    </Ico>
  );
}

export function IconHome(p: Props) {
  return (
    <Ico {...p}>
      <path
        d="M4 10.5 12 4l8 6.5V20a1 1 0 0 1-1 1h-5v-6H10v6H5a1 1 0 0 1-1-1v-9.5Z"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinejoin="round"
      />
    </Ico>
  );
}

export function IconPlus(p: Props) {
  return (
    <Ico {...p}>
      <path
        d="M12 5v14M5 12h14"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
      />
    </Ico>
  );
}

export function IconMinus(p: Props) {
  return (
    <Ico {...p}>
      <path d="M5 12h14" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" />
    </Ico>
  );
}

export function IconEye(p: Props) {
  return (
    <Ico {...p}>
      <path
        d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinejoin="round"
      />
      <circle cx="12" cy="12" r="3" stroke="currentColor" strokeWidth="1.8" />
    </Ico>
  );
}

export function IconEyeOff(p: Props) {
  return (
    <Ico {...p}>
      <path
        d="M3 3l18 18M10.6 10.6a2 2 0 0 0 2.8 2.8M9.9 5.2A10.4 10.4 0 0 1 12 5c6.5 0 10 7 10 7a18.4 18.4 0 0 1-2.4 3.2M6.1 6.1C3.7 7.8 2 12 2 12s3.5 7 10 7c1.4 0 2.7-.3 3.9-.8"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Ico>
  );
}

export function IconRefresh(p: Props) {
  return (
    <Ico {...p}>
      <path
        d="M20 12a8 8 0 1 1-2.3-5.6M20 4v5h-5"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Ico>
  );
}

/** Official Trezor logomark (via simple-icons.org), used at brand scale for device status. */
export function IconTrezorDevice({ size = 18, className }: Props) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="currentColor"
      className={className}
      aria-hidden
    >
      <path d="M17.858 5.569c0-3.044-2.643-5.569-5.86-5.569-3.216 0-5.856 2.525-5.856 5.569v1.78H3.731V20.15L11.998 24l8.271-3.849V7.403h-2.411zm-8.73 0c0-1.434 1.264-2.584 2.87-2.584 1.61 0 2.87 1.15 2.87 2.584v1.78h-5.74Zm7.81 12.516-4.94 2.298-4.937-2.298v-7.693h9.878z" />
    </svg>
  );
}

export function IconChevronDown(p: Props) {
  return (
    <Ico {...p}>
      <path
        d="M6 9l6 6 6-6"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Ico>
  );
}

export function IconExternalLink(p: Props) {
  return (
    <Ico {...p}>
      <path
        d="M14 5h5v5M19 5l-9 9M10 5H6a1 1 0 0 0-1 1v12a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-4"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </Ico>
  );
}

export function IconCopy(p: Props) {
  return (
    <Ico {...p}>
      <rect
        x="8"
        y="8"
        width="11"
        height="11"
        rx="2"
        stroke="currentColor"
        strokeWidth="1.8"
      />
      <path
        d="M6 15V6a2 2 0 0 1 2-2h9"
        stroke="currentColor"
        strokeWidth="1.8"
        strokeLinecap="round"
      />
    </Ico>
  );
}
