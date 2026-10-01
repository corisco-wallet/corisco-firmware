// BTC/fiat conversion for the Home screen's "below the balance" line --
// display-only, fetched from a public API with no key required. This is
// purely informational (the wallet itself, and the regtest network it
// talks to, don't need or use a fiat rate for anything) -- a failed fetch
// just means the conversion line doesn't show, not a broken wallet.

const API_URL = "https://api.coingecko.com/api/v3/simple/price";

/** Fetches the current price of 1 BTC in `currency` (e.g. "usd"). Returns
 * `null` on any failure (offline, rate-limited, unexpected response
 * shape) rather than throwing -- callers treat that as "no conversion
 * available right now," not an error to surface to the user. */
export async function fetchBtcPrice(currency: string): Promise<number | null> {
  try {
    const res = await fetch(`${API_URL}?ids=bitcoin&vs_currencies=${encodeURIComponent(currency)}`);
    if (!res.ok) return null;
    const json = (await res.json()) as { bitcoin?: Record<string, number> };
    const price = json.bitcoin?.[currency];
    return typeof price === "number" ? price : null;
  } catch {
    return null;
  }
}

const SATS_PER_BTC = 100_000_000;

export function satsToBtc(sats: bigint): number {
  return Number(sats) / SATS_PER_BTC;
}

export function satsToFiat(sats: bigint, btcPrice: number): number {
  return satsToBtc(sats) * btcPrice;
}

/** `Intl.NumberFormat`'s `currency` option wants an uppercase ISO code. */
export function formatFiat(amount: number, currency: string): string {
  try {
    return new Intl.NumberFormat(undefined, {
      style: "currency",
      currency: currency.toUpperCase(),
      maximumFractionDigits: 2,
    }).format(amount);
  } catch {
    return `${amount.toFixed(2)} ${currency.toUpperCase()}`;
  }
}

export function formatBtc(sats: bigint): string {
  return satsToBtc(sats).toFixed(8);
}
