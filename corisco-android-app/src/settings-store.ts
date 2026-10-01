// Persisted, per-phone display preferences -- currency, which unit the
// balance is shown in, and whether amounts are hidden for privacy. None of
// this affects wallet behavior or signing (see App.tsx's top doc comment
// on key isolation); it's purely how numbers already fetched from the
// wallet get displayed.

import AsyncStorage from "@react-native-async-storage/async-storage";

const STORAGE_KEY = "corisco.settings";

export type BalanceUnit = "sats" | "btc";

export type Settings = {
  /** Lowercase ISO 4217-ish code, e.g. "usd" -- matches what the price API
   * (see price.ts) and `Intl.NumberFormat`'s `currency` option expect. */
  currency: string;
  balanceUnit: BalanceUnit;
  hideAmounts: boolean;
  showLastXTransactions: number;
};

export const DEFAULT_SETTINGS: Settings = {
  currency: "usd",
  balanceUnit: "sats",
  hideAmounts: false,
  showLastXTransactions: 3,
};

/** A small curated list rather than every ISO code CoinGecko supports --
 * this is a wallet home-screen picker, not a currency database. Add more
 * here freely; nothing else needs to change (see price.ts). */
export const CURRENCIES: { code: string; label: string }[] = [
  { code: "usd", label: "USD" },
  { code: "eur", label: "EUR" },
  { code: "gbp", label: "GBP" },
  { code: "jpy", label: "JPY" },
  { code: "brl", label: "BRL" },
  { code: "cad", label: "CAD" },
  { code: "aud", label: "AUD" },
  { code: "chf", label: "CHF" },
];

/** Fixed choices for `showLastXTransactions` -- a plain number input would
 * let someone type e.g. 500 and make every refresh fetch a huge history;
 * a small picker keeps it to sane values (same reasoning as `CURRENCIES`
 * being curated rather than open-ended). */
export const TRANSACTION_COUNT_OPTIONS: number[] = [3, 5, 10, 20, 50];

export async function loadSettings(): Promise<Settings> {
  const raw = await AsyncStorage.getItem(STORAGE_KEY);
  if (!raw) return DEFAULT_SETTINGS;
  try {
    const parsed = JSON.parse(raw);
    return { ...DEFAULT_SETTINGS, ...parsed };
  } catch {
    return DEFAULT_SETTINGS;
  }
}

export async function saveSettings(settings: Settings): Promise<void> {
  await AsyncStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
}
