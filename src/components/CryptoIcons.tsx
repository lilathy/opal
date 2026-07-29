/** Crypto logos: Exodus's own badge art (self-contained hex shape) for the
 * coins we have locally, falling back to MIT-licensed @web3icons/core
 * network glyphs for the newer L2 chains Exodus's export didn't include. */
import { useMemo } from "react";
import exBtc from "../assets/exodus/btc.svg?raw";
import exEth from "../assets/exodus/eth.svg?raw";
import exSol from "../assets/exodus/sol.svg?raw";
import exLtc from "../assets/exodus/ltc.svg?raw";
import exDoge from "../assets/exodus/doge.svg?raw";
import exXmr from "../assets/exodus/xmr.svg?raw";
import exTrx from "../assets/exodus/trx.svg?raw";
import exTon from "../assets/exodus/ton.svg?raw";
import exUsdc from "../assets/exodus/usdc.svg?raw";
import exUsdt from "../assets/exodus/usdt.svg?raw";
import exDai from "../assets/exodus/dai.svg?raw";
import exWbtc from "../assets/exodus/wbtc.svg?raw";
import exWeth from "../assets/exodus/weth.svg?raw";
import exPolygon from "../assets/exodus/polygon.svg?raw";
import exMatic from "../assets/exodus/matic.svg?raw";
import exBase from "../assets/exodus/base.svg?raw";
import exAvax from "../assets/exodus/avax.svg?raw";
import exBnb from "../assets/exodus/bnb.svg?raw";

import netArbitrum from "@web3icons/core/svgs/networks/background/arbitrum-one.svg.js";
import netGnosis from "@web3icons/core/svgs/networks/background/gnosis.svg.js";
import netLinea from "@web3icons/core/svgs/networks/background/linea.svg.js";
import tokenGNO from "@web3icons/core/svgs/tokens/background/GNO.svg.js";

export const CHAIN_COLORS: Record<string, string> = {
  btc: "#F7931A",
  eth: "#627EEA",
  arb: "#28A0F0",
  base: "#0052FF",
  polygon: "#8247E5",
  avax: "#E84142",
  bsc: "#F0B90B",
  gnosis: "#04795B",
  trx: "#FF0013",
  linea: "#61DFFF",
  sol: "#9945FF",
  ton: "#0098EA",
  ltc: "#345D9D",
  doge: "#C2A633",
  xmr: "#FF6600",
};

export const ASSET_COLORS: Record<string, string> = {
  ...CHAIN_COLORS,
  usdc: "#2775CA",
  usdt: "#26A17B",
  dai: "#F5AC37",
  weth: "#627EEA",
  wbtc: "#F7931A",
};

/** Exodus's badge art already bakes in its own hex-shaped background, so it
 * must render at native size with no extra clipping. The @web3icons
 * fallbacks are flat colored squares that still need the rounded clip. */
type IconEntry = { markup: string; shaped: boolean };
const shaped = (markup: string): IconEntry => ({ markup, shaped: true });
const unshaped = (markup: string): IconEntry => ({ markup, shaped: false });

/** Chain "badge" icon - one per network we support adding a portfolio for. */
const CHAIN_SRC: Record<string, IconEntry> = {
  btc: shaped(exBtc),
  eth: shaped(exEth),
  sol: shaped(exSol),
  ltc: shaped(exLtc),
  doge: shaped(exDoge),
  xmr: shaped(exXmr),
  trx: shaped(exTrx),
  ton: shaped(exTon),
  polygon: shaped(exPolygon),
  avax: shaped(exAvax),
  bsc: shaped(exBnb),
  base: shaped(exBase),
  // Kept for existing portfolios on networks no longer offered in Add Portfolio.
  arb: unshaped(netArbitrum),
  gnosis: unshaped(netGnosis),
  linea: unshaped(netLinea),
};

/** Per-asset ticker icon (native coins + common tokens held inside a portfolio). */
const ASSET_SRC: Record<string, IconEntry> = {
  ...CHAIN_SRC,
  usdc: shaped(exUsdc),
  usdt: shaped(exUsdt),
  dai: shaped(exDai),
  wbtc: shaped(exWbtc),
  weth: shaped(exWeth),
  bnb: shaped(exBnb),
  matic: shaped(exMatic),
  pol: shaped(exPolygon),
  xdai: unshaped(netGnosis),
  wxdai: unshaped(netGnosis),
  gno: unshaped(tokenGNO),
};

type SvgIconProps = {
  entry: IconEntry;
  size: number;
  className?: string;
};

let iconInstanceCounter = 0;

/** Several of the source SVGs reuse generic internal ids (gradients,
 * clip-paths) like `id="a"`. Fine for one icon on the page, but as soon as
 * the same coin shows up twice (sidebar + overview, a list of holdings...)
 * the duplicate ids collide in the document and the *second* instance's
 * `url(#a)` / `href="#a"` references silently resolve to the first one's
 * (or nothing), leaving it unstyled or blank. Namespace every id per render. */
function namespaceIds(markup: string, suffix: string): string {
  const ids = new Set<string>();
  const idRe = /\bid="([^"]+)"/g;
  let m: RegExpExecArray | null;
  while ((m = idRe.exec(markup))) ids.add(m[1]);
  if (ids.size === 0) return markup;
  let out = markup;
  for (const id of ids) {
    const esc = id.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    out = out
      .replace(new RegExp(`id="${esc}"`, "g"), `id="${id}-${suffix}"`)
      .replace(new RegExp(`url\\(#${esc}\\)`, "g"), `url(#${id}-${suffix})`)
      .replace(new RegExp(`(href="#)${esc}(")`, "g"), `$1${id}-${suffix}$2`);
  }
  return out;
}

/** Ensure the SVG scales inside our sized container. Missing viewBox + fixed
 * width/height (e.g. Gram/TON) overflow list rows; strip intrinsic size and
 * fall back to a 40×40 viewBox so CSS can own layout. */
function fitSvg(markup: string): string {
  let out = markup;
  if (!/\bviewBox\s*=/.test(out)) {
    out = out.replace(/<svg\b/, '<svg viewBox="0 0 40 40"');
  }
  out = out.replace(/<svg\b([^>]*)>/, (_m, attrs: string) => {
    const cleaned = attrs.replace(/\s(?:width|height)\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/g, "");
    return `<svg${cleaned}>`;
  });
  return out;
}

/** Renders a pre-optimized inline SVG string at an exact pixel size. Shaped
 * (Exodus) art carries its own badge outline and renders untouched; the
 * flat network-square fallbacks get a soft rounded-square clip instead. */
function CryptoSvg({ entry, size, className }: SvgIconProps) {
  const suffix = useMemo(() => `ci${(iconInstanceCounter++).toString(36)}`, []);
  const markup = useMemo(
    () => fitSvg(namespaceIds(entry.markup, suffix)),
    [entry.markup, suffix],
  );
  const cls = entry.shaped
    ? "crypto-img crypto-img--shaped"
    : "crypto-img crypto-img--clip";
  return (
    <span
      className={className ? `${cls} ${className}` : cls}
      style={{ width: size, height: size }}
      // Trusted, bundled, static icon markup - never user-controlled.
      dangerouslySetInnerHTML={{ __html: markup }}
    />
  );
}

function FallbackBadge({
  label,
  size,
  color,
  className,
}: {
  label: string;
  size: number;
  color: string;
  className?: string;
}) {
  return (
    <span
      className={className ? `crypto-fallback ${className}` : "crypto-fallback"}
      style={{
        width: size,
        height: size,
        background: color,
        fontSize: Math.max(9, Math.round(size * 0.32)),
      }}
      aria-hidden
    >
      {(label ?? "?").slice(0, 3).toUpperCase()}
    </span>
  );
}

export function ChainIcon({
  chain,
  size = 28,
  className,
}: {
  chain: string;
  size?: number;
  className?: string;
}) {
  const id = (chain ?? "").toLowerCase();
  const entry = CHAIN_SRC[id];
  if (entry) {
    return <CryptoSvg entry={entry} size={size} className={className} />;
  }
  return (
    <FallbackBadge
      label={id}
      size={size}
      color={CHAIN_COLORS[id] ?? "#555"}
      className={className}
    />
  );
}

export function AssetIcon({
  symbol,
  size = 28,
  className,
}: {
  symbol: string;
  size?: number;
  className?: string;
}) {
  const id = (symbol ?? "").toLowerCase();
  const entry = ASSET_SRC[id];
  if (entry) {
    return <CryptoSvg entry={entry} size={size} className={className} />;
  }
  return (
    <FallbackBadge
      label={symbol}
      size={size}
      color={ASSET_COLORS[id] ?? "#555555"}
      className={className}
    />
  );
}

export function chainTint(chain: string): string {
  return CHAIN_COLORS[(chain ?? "").toLowerCase()] ?? "#8f8f8f";
}
