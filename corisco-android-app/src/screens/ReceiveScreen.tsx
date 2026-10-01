// Generate a Lightning invoice -- automatically on open (any amount), and
// again whenever the user types a fixed amount -- show it as a QR code,
// and let the user copy the raw invoice string.
//
// BTC L1 receiving (a static deposit address) was tried and deliberately
// removed: generating the address only needs a public key (safe), but
// actually claiming a confirmed deposit into spendable balance requires
// the SDK to call `signer.getStaticDepositSecretKey()`, which hands over
// the *raw private key* -- unlike every other operation this signer
// exposes (signatures, Shamir shares, ECIES ciphertext), never a raw key.
// An address with no way to ever claim what's sent to it would just
// strand real funds, so the whole feature was pulled rather than shipped
// half-working.

import { useEffect, useState } from "react";
import {
  ActivityIndicator,
  StyleSheet,
  Text,
  TextInput,
  TouchableOpacity,
  View,
} from "react-native";
import * as Clipboard from "expo-clipboard";
import QRCode from "react-native-qrcode-svg";
import Svg, { Path, Polyline, Rect } from "react-native-svg";
import type { SparkWallet as SparkWalletType } from "@buildonspark/spark-sdk";
import { colors, radii, spacing } from "../theme";

// Hand-drawn (Feather-style) icons via react-native-svg -- already a
// dependency for the QR code above -- rather than an emoji glyph, which
// rendered as an odd boxed/monochrome symbol on Android instead of a
// clean picture.
function CopyIcon({ color, size = 20 }: { color: string; size?: number }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
      <Rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
      <Path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
    </Svg>
  );
}

function CheckIcon({ color, size = 20 }: { color: string; size?: number }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
      <Polyline points="20 6 9 17 4 12" />
    </Svg>
  );
}

export function ReceiveScreen({ wallet, onBack }: { wallet: SparkWalletType; onBack: () => void }) {
  const [amountText, setAmountText] = useState("");
  const [invoice, setInvoice] = useState<string | null>(null);
  // The amount the *currently displayed* invoice was actually generated
  // for -- kept separate from `amountText` so the on-screen label always
  // reflects what the QR code really encodes, not whatever's mid-typing
  // in the field a moment before the debounced regenerate catches up.
  const [invoiceAmountSats, setInvoiceAmountSats] = useState<number | null>(null);
  const [generating, setGenerating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  // Generates an invoice for `amountSats` (0 = any amount) and, only if
  // this is still the most recent request in flight, shows it -- guards
  // against an in-flight regenerate for an amount the user has since
  // changed again from clobbering a newer one that resolves first.
  useEffect(() => {
    let cancelled = false;
    const amountSats = amountText.trim() === "" ? 0 : Math.floor(Number(amountText));
    if (Number.isNaN(amountSats) || amountSats < 0) {
      setError("Enter a valid amount, or leave it blank for any amount");
      return;
    }
    setGenerating(true);
    setError(null);
    // No debounce on the very first (any-amount) generation -- this is
    // what makes an invoice appear the instant the screen opens, with no
    // tap required. Only typing an amount afterward debounces.
    const delay = amountText.trim() === "" && invoice === null ? 0 : 400;
    const timer = setTimeout(() => {
      wallet
        .createLightningInvoice({ amountSats, memo: "Corisco" })
        .then((request) => {
          if (cancelled) return;
          setInvoice(request.invoice.encodedInvoice);
          setInvoiceAmountSats(amountSats);
        })
        .catch((err) => {
          if (cancelled) return;
          setError(String(err));
        })
        .finally(() => {
          if (!cancelled) setGenerating(false);
        });
    }, delay);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [amountText, wallet]);

  const copy = async () => {
    if (!invoice) return;
    await Clipboard.setStringAsync(invoice);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <View style={styles.container}>
      <TouchableOpacity onPress={onBack} style={styles.backButton}>
        <Text style={styles.backText}>{"< Back"}</Text>
      </TouchableOpacity>

      <Text style={styles.title}>Receive</Text>

      <Text style={styles.label}>Amount (sats) -- optional</Text>
      <TextInput
        style={styles.input}
        placeholder="Any amount"
        placeholderTextColor={colors.textMuted}
        value={amountText}
        onChangeText={setAmountText}
        keyboardType="number-pad"
      />

      {invoice && !error && (
        <View style={styles.invoiceBlock}>
          <View style={styles.qrWrap}>
            <QRCode value={invoice} size={210} backgroundColor="#FFFFFF" color="#000000" />
          </View>
          <Text style={styles.amountLabel}>
            {invoiceAmountSats && invoiceAmountSats > 0 ? `${invoiceAmountSats} sats` : "Any amount"}
            {generating && "  (updating...)"}
          </Text>
          <Text style={styles.invoiceText} numberOfLines={3} ellipsizeMode="middle">
            {invoice}
          </Text>
          <TouchableOpacity style={styles.copyButton} onPress={copy}>
            {copied ? <CheckIcon color={colors.success} /> : <CopyIcon color={colors.textPrimary} />}
          </TouchableOpacity>
        </View>
      )}
      {!invoice && generating && (
        <View style={styles.loadingBlock}>
          <ActivityIndicator color={colors.accent} size="large" />
          <Text style={styles.loadingText}>Generating invoice...</Text>
        </View>
      )}
      {error && <Text style={styles.error}>{error}</Text>}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: colors.background,
    padding: spacing.lg,
    paddingTop: 60,
  },
  backButton: {
    marginBottom: spacing.lg,
  },
  backText: {
    color: colors.textSecondary,
    fontSize: 16,
  },
  title: {
    color: colors.textPrimary,
    fontSize: 28,
    fontWeight: "700",
    marginBottom: spacing.xl,
  },
  label: {
    color: colors.textSecondary,
    fontSize: 14,
    marginBottom: spacing.sm,
  },
  input: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radii.md,
    padding: spacing.md,
    color: colors.textPrimary,
    fontSize: 18,
    marginBottom: spacing.lg,
  },
  copyButton: {
    width: 48,
    height: 48,
    borderRadius: radii.pill,
    borderWidth: 1,
    borderColor: colors.border,
    backgroundColor: colors.surface,
    alignItems: "center",
    justifyContent: "center",
    marginTop: spacing.sm,
  },
  error: {
    color: colors.error,
    marginTop: spacing.md,
    textAlign: "center",
  },
  invoiceBlock: {
    alignItems: "center",
  },
  loadingBlock: {
    alignItems: "center",
    paddingVertical: spacing.xl,
  },
  loadingText: {
    color: colors.textSecondary,
    fontSize: 14,
    marginTop: spacing.md,
  },
  qrWrap: {
    backgroundColor: "#FFFFFF",
    padding: spacing.md,
    borderRadius: radii.lg,
    marginBottom: spacing.md,
  },
  amountLabel: {
    color: colors.textPrimary,
    fontSize: 18,
    fontWeight: "700",
    marginBottom: spacing.sm,
  },
  invoiceText: {
    color: colors.textSecondary,
    fontSize: 13,
    fontFamily: "monospace",
    textAlign: "center",
    marginBottom: spacing.lg,
    paddingHorizontal: spacing.sm,
  },
});
