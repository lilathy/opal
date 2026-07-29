import { useAnimatedNumber } from "../hooks/useAnimatedNumber";
import { formatMoney } from "../lib/format";

type Tag = "span" | "p";

type Props = {
  value: number;
  fiat: string;
  discreet: boolean;
  className?: string;
  as?: Tag;
  /**
   * Identity for this readout (portfolio id, screen, asset, …).
   * When it changes, jump immediately - no count-up across navigation.
   */
  snapKey?: string | number | boolean | null;
};

/** Fiat amount that glides between values instead of jumping. */
export function AnimatedMoney({
  value,
  fiat,
  discreet,
  className,
  as: Tag = "span",
  snapKey = null,
}: Props) {
  const animated = useAnimatedNumber(value, {
    enabled: !discreet,
    snapKey: `${fiat}:${discreet ? 1 : 0}:${snapKey ?? ""}`,
    durationMs: 880,
  });

  return (
    <Tag className={className}>
      {discreet ? "••••" : formatMoney(animated, fiat, false)}
    </Tag>
  );
}
