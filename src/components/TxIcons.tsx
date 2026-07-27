import type { ReactNode } from "react";

type Props = {
  size?: number;
  className?: string;
};

function Hex({
  size = 32,
  className,
  fill,
  children,
}: Props & { fill: string; children: ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 40 40"
      xmlns="http://www.w3.org/2000/svg"
      className={className}
      aria-hidden
    >
      <g fill="none" fillRule="evenodd">
        <path
          d="M22 1.155l13.32 7.69a4 4 0 0 1 2 3.464v15.382a4 4 0 0 1-2 3.464L22 38.845a4 4 0 0 1-4 0l-13.32-7.69a4 4 0 0 1-2-3.464V12.309a4 4 0 0 1 2-3.464L18 1.155a4 4 0 0 1 4 0z"
          fill={fill}
        />
        <g
          stroke="#FFFFFF"
          strokeWidth="2.8"
          strokeLinecap="round"
          strokeLinejoin="round"
          opacity="0.95"
        >
          {children}
        </g>
      </g>
    </svg>
  );
}

/** Sent — uses app loss/negative red. */
export function TxIconSent({ size = 32, className }: Props) {
  return (
    <Hex size={size} className={className} fill="#f87171">
      <line x1="15" y1="25" x2="25" y2="15" />
      <polyline points="15 15 25 15 25 25" />
    </Hex>
  );
}

/** Received — uses app profit/positive green. */
export function TxIconReceived({ size = 32, className }: Props) {
  return (
    <Hex size={size} className={className} fill="#34d399">
      <line x1="25" y1="15" x2="15" y2="25" />
      <polyline points="25 25 15 25 15 15" />
    </Hex>
  );
}

/** Self-transfer — muted stone. */
export function TxIconSelf({ size = 32, className }: Props) {
  return (
    <Hex size={size} className={className} fill="#8a857e">
      <path d="M14 16h12M26 16l-3-3M26 16l-3 3M26 24H14M14 24l3-3M14 24l3 3" />
    </Hex>
  );
}
