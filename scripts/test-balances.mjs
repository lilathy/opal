/**
 * Run: node scripts/test-balances.mjs
 * Verifies fiat price resolution without network (logic-only).
 */

function normFiat(fiat) {
  return fiat.trim().toUpperCase();
}

const fiatPriceCache = new Map();
const fiatBtcRates = new Map();

function cacheFiatPrices(fiat, prices) {
  fiatPriceCache.set(normFiat(fiat), prices);
  if (prices.bitcoin != null) fiatBtcRates.set(normFiat(fiat), prices.bitcoin);
}

function scalePriceMap(prices, ratio) {
  const out = {};
  for (const [k, v] of Object.entries(prices)) out[k] = v * ratio;
  return out;
}

function resolvePricesForFiat(fiat) {
  const target = normFiat(fiat);
  const direct = fiatPriceCache.get(target);
  if (direct && Object.keys(direct).length > 0) return direct;

  const targetBtc = fiatBtcRates.get(target);
  if (targetBtc != null) {
    for (const [cachedFiat, prices] of fiatPriceCache) {
      const baseBtc = fiatBtcRates.get(cachedFiat) ?? prices.bitcoin;
      if (!baseBtc) continue;
      return scalePriceMap(prices, targetBtc / baseBtc);
    }
  }

  const usd = fiatPriceCache.get("USD");
  const usdBtc = fiatBtcRates.get("USD") ?? usd?.bitcoin;
  if (usd && usdBtc && targetBtc != null) {
    return scalePriceMap(usd, targetBtc / usdBtc);
  }
  return {};
}

// Seed only USD (simulates first app load before warm completes)
cacheFiatPrices("USD", { bitcoin: 100000, solana: 200, ethereum: 4000 });

// Simulate warm filling only BTC pivot rates for other fiats (partial warm)
fiatBtcRates.set("EUR", 92000);
fiatBtcRates.set("GBP", 78000);

const eur = resolvePricesForFiat("EUR");
const gbp = resolvePricesForFiat("GBP");

let failed = 0;
function assert(cond, msg) {
  if (!cond) {
    console.error("FAIL:", msg);
    failed += 1;
  } else {
    console.log("ok:", msg);
  }
}

assert(eur.solana === 184, `EUR solana = 184 got ${eur.solana}`);
assert(gbp.solana === 156, `GBP solana = 156 got ${gbp.solana}`);
assert(eur.bitcoin === 92000, `EUR bitcoin pivot`);

// Direct cache path
cacheFiatPrices("EUR", { bitcoin: 92000, solana: 184, ethereum: 3680 });
const eurDirect = resolvePricesForFiat("EUR");
assert(eurDirect.solana === 184, "direct EUR cache");

// Exchange price-book smoke (Binance + FX) — optional network
const t0 = Date.now();
const [ticksRes, fxRes] = await Promise.all([
  fetch("https://api.binance.com/api/v3/ticker/price?symbol=SOLUSDT"),
  fetch("https://open.er-api.com/v6/latest/USD"),
]);
const tick = await ticksRes.json();
const fx = await fxRes.json();
const ms = Date.now() - t0;
const sol = Number(tick.price);
const eurRate = Number(fx?.rates?.EUR);
assert(ticksRes.ok && sol > 0, `binance SOL ${sol} in ${ms}ms`);
assert(fxRes.ok && eurRate > 0, `fx EUR ${eurRate}`);

if (failed) {
  console.error(`${failed} test(s) failed`);
  process.exit(1);
}
console.log("all balance/fiat tests passed");
