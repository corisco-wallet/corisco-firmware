// Pay a Lightning invoice, either pasted in directly or scanned from a QR
// code.

import { useEffect, useState } from "react";
import {
  ActivityIndicator,
  Keyboard,
  StyleSheet,
  Text,
  TextInput,
  TouchableOpacity,
  View,
} from "react-native";
import { CameraView, useCameraPermissions } from "expo-camera";
import { decode as decodeBolt11 } from "light-bolt11-decoder";
import type { SparkWallet as SparkWalletType } from "@buildonspark/spark-sdk";
import type { BleHardwareSigner } from "../ble-hardware-signer";
import { colors, radii, spacing } from "../theme";
import { SendingScreen } from "./SendingScreen";
import { SendResultScreen } from "./SendResultScreen";

/** Shortens the raw invoice string for the device's small screen -- same
 * head+tail truncation style `esp32-lilygo-t-display-s3-firmware/src/main.rs`'s
 * `short_id` already uses for leaf ids. Deliberately the invoice itself, not its
 * bolt11 description: a description is arbitrary, untrusted memo text the
 * payee chose (doesn't identify *what's being paid* at all, and could be
 * written to look like something it isn't), whereas a fragment of the
 * actual invoice at least lets a human visually match the confirm screen
 * against the invoice/QR code they meant to pay. */
function shortInvoice(invoice: string): string {
  return invoice.length <= 22 ? invoice : `${invoice.slice(0, 12)}…${invoice.slice(-8)}`;
}

/** Pulls the amount (sats) out of a bolt11 invoice, for
 * `BleHardwareSigner.withSpendContext` -- shown on the device's own
 * confirm screen (see that method's doc comment on why this is
 * app-asserted, not independently verified against what's actually
 * signed). `null` means a 0-amount/any-amount invoice; matches
 * `amountSatsToSend` being required in that case (see `pay` below). */
function decodeInvoiceAmountSats(invoice: string): bigint | null {
  const { sections } = decodeBolt11(invoice);
  for (const section of sections) {
    if (section.name === "amount") return BigInt(section.value) / 1000n;
  }
  return null;
}

export function SendScreen({
  wallet,
  signer,
  onBack,
  onPaid,
}: {
  wallet: SparkWalletType;
  signer: BleHardwareSigner;
  onBack: () => void;
  onPaid: () => void;
}) {
  const [invoice, setInvoice] = useState("");
  const [amountText, setAmountText] = useState("");
  // The amount a *fixed-amount* invoice carries, decoded as the invoice
  // text changes -- `null` means either the invoice hasn't been
  // (successfully) decoded yet, or it's a 0-amount/any-amount invoice, in
  // both of which cases the amount field stays user-editable. Once an
  // invoice with its own amount is recognized, the field locks to that
  // value: the SDK's `payLightningInvoice` only accepts an explicit
  // `amountSatsToSend` for a 0-amount invoice (see its own doc comment) --
  // sending one alongside a fixed-amount invoice isn't a real option, so
  // there's nothing meaningful to type there.
  const [invoiceAmountSats, setInvoiceAmountSats] = useState<bigint | null>(null);
  const [scanning, setScanning] = useState(false);
  const [paying, setPaying] = useState(false);
  // Full-screen result shown once `pay()` finishes, success or failure --
  // distinct from `error` below, which is only ever a pre-flight/form
  // issue (e.g. camera permission for the QR scanner), not a payment
  // outcome.
  const [payOutcome, setPayOutcome] = useState<{ ok: boolean; message: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [permission, requestPermission] = useCameraPermissions();

  const handleInvoiceChange = (text: string) => {
    setInvoice(text);
    setError(null);
    const trimmed = text.trim();
    if (!trimmed) {
      setInvoiceAmountSats(null);
      return;
    }
    try {
      const decoded = decodeInvoiceAmountSats(trimmed);
      setInvoiceAmountSats(decoded);
      if (decoded !== null) setAmountText(decoded.toString());
    } catch {
      // Not yet a decodable invoice (still mid-paste, or garbage) --
      // leave whatever's already in the amount field alone.
      setInvoiceAmountSats(null);
    }
  };

  const startScan = async () => {
    if (!permission?.granted) {
      const res = await requestPermission();
      if (!res.granted) {
        setError("Camera permission is needed to scan a QR code.");
        return;
      }
    }
    setError(null);
    setScanning(true);
  };

  const onBarcodeScanned = ({ data }: { data: string }) => {
    setScanning(false);
    handleInvoiceChange(data.trim());
  };

  // What amount actually gets sent: the invoice's own fixed amount if it
  // has one, otherwise whatever's typed into the amount field (for a
  // 0-amount invoice, where the payer chooses).
  const manualAmountSats = (() => {
    const parsed = Math.floor(Number(amountText.trim()));
    return amountText.trim() !== "" && Number.isFinite(parsed) && parsed > 0 ? BigInt(parsed) : null;
  })();
  const effectiveAmountSats = invoiceAmountSats ?? manualAmountSats;

  // Real fee estimate (not a guess) from the SDK, refetched whenever the
  // invoice or amount changes -- debounced since amount typing (for a
  // 0-amount invoice) fires on every keystroke. Failure just hides the
  // fee/total line rather than blocking anything -- it's informational,
  // same "degrade gracefully" treatment as HomeScreen's BTC/fiat rate.
  const [feeEstimateSats, setFeeEstimateSats] = useState<bigint | null>(null);
  const [feeLoading, setFeeLoading] = useState(false);
  const [feeError, setFeeError] = useState<string | null>(null);

  useEffect(() => {
    const trimmed = invoice.trim();
    if (!trimmed || effectiveAmountSats === null) {
      setFeeEstimateSats(null);
      setFeeError(null);
      setFeeLoading(false);
      return;
    }
    let cancelled = false;
    setFeeLoading(true);
    const timer = setTimeout(() => {
      wallet
        .getLightningSendFeeEstimate({
          encodedInvoice: trimmed,
          // Same convention as `payLightningInvoice`'s `amountSatsToSend`
          // below -- only meaningful (and only accepted) for a 0-amount
          // invoice.
          ...(invoiceAmountSats === null ? { amountSats: Number(effectiveAmountSats) } : {}),
        })
        .then((fee) => {
          if (cancelled) return;
          setFeeEstimateSats(BigInt(Math.ceil(fee)));
          setFeeError(null);
        })
        .catch(() => {
          if (cancelled) return;
          setFeeEstimateSats(null);
          setFeeError("Fee estimate unavailable");
        })
        .finally(() => {
          if (!cancelled) setFeeLoading(false);
        });
    }, 400);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [invoice, effectiveAmountSats, invoiceAmountSats, wallet]);

  const pay = async () => {
    Keyboard.dismiss();
    const trimmed = invoice.trim();
    if (!trimmed || effectiveAmountSats === null) return;
    setPaying(true);
    setError(null);
    try {
      // Every Sign this triggers requires on-device confirmation -- see
      // `withSpendContext`'s doc comment for why it doesn't try to filter
      // out any of them (a real spend silently went through with none
      // shown at all, when this tried inferring which one to skip).
      const payment = await signer.withSpendContext(
        { amountSats: effectiveAmountSats, destination: shortInvoice(trimmed) },
        () =>
          wallet.payLightningInvoice({
            invoice: trimmed,
            // The real estimate (plus a small margin) when we have one,
            // rather than an arbitrary flat cap -- falls back to 100 sats
            // if the estimate never came back.
            maxFeeSats: feeEstimateSats !== null ? Number(feeEstimateSats) + 10 : 100,
            preferSpark: false,
            // Only set (and only accepted by the SDK) for a 0-amount
            // invoice -- a fixed-amount invoice already carries its own.
            ...(invoiceAmountSats === null ? { amountSatsToSend: Number(effectiveAmountSats) } : {}),
          }),
      );
      setPayOutcome({ ok: true, message: `${effectiveAmountSats.toString()} sats sent -- status: ${payment.status}` });
      onPaid();
    } catch (err) {
      setPayOutcome({ ok: false, message: String(err) });
    } finally {
      setPaying(false);
    }
  };

  const retry = () => setPayOutcome(null);

  const finish = () => {
    setPayOutcome(null);
    onBack();
  };

  // Full-screen, not an inline spinner -- a payment can be waiting on a
  // physical tap on the hardware signer's own confirm screen, which is
  // easy to miss as "still loading" versus "actually stuck" without a
  // more deliberate status screen (same reasoning as SyncingScreen's
  // wallet-load indicator).
  if (paying) {
    return <SendingScreen label="Sending payment..." />;
  }

  if (payOutcome) {
    return (
      <SendResultScreen
        ok={payOutcome.ok}
        message={payOutcome.message}
        onDone={finish}
        onRetry={payOutcome.ok ? undefined : retry}
      />
    );
  }

  if (scanning) {
    return (
      <View style={styles.scannerContainer}>
        <CameraView
          style={StyleSheet.absoluteFill}
          facing="back"
          barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
          onBarcodeScanned={onBarcodeScanned}
        />
        <View style={styles.scannerOverlay}>
          <View style={styles.scannerFrame} />
          <TouchableOpacity style={styles.cancelScanButton} onPress={() => setScanning(false)}>
            <Text style={styles.secondaryButtonText}>Cancel</Text>
          </TouchableOpacity>
        </View>
      </View>
    );
  }

  return (
    <View style={styles.container}>
      <TouchableOpacity onPress={onBack} style={styles.backButton}>
        <Text style={styles.backText}>{"< Back"}</Text>
      </TouchableOpacity>

      <Text style={styles.title}>Send</Text>

      <Text style={styles.label}>Lightning invoice</Text>
      <TextInput
        style={styles.input}
        placeholder="lnbc..."
        placeholderTextColor={colors.textMuted}
        value={invoice}
        onChangeText={handleInvoiceChange}
        autoCapitalize="none"
        autoCorrect={false}
        multiline
      />

      <TouchableOpacity style={styles.secondaryButtonOutline} onPress={startScan}>
        <Text style={styles.secondaryButtonOutlineText}>Scan QR code</Text>
      </TouchableOpacity>

      <Text style={styles.label}>Amount (sats)</Text>
      <TextInput
        style={[styles.amountInput, invoiceAmountSats !== null && styles.amountInputLocked]}
        placeholder="Enter amount"
        placeholderTextColor={colors.textMuted}
        value={amountText}
        onChangeText={setAmountText}
        keyboardType="number-pad"
        editable={invoiceAmountSats === null}
      />
      {invoice.trim() !== "" && (
        <Text style={styles.hint}>
          {invoiceAmountSats !== null ? "Amount set by the invoice." : "This invoice doesn't set an amount -- enter one."}
        </Text>
      )}

      {invoice.trim() !== "" && effectiveAmountSats !== null && (
        <View style={styles.feeBlock}>
          {feeLoading ? (
            <ActivityIndicator size="small" color={colors.textMuted} />
          ) : feeEstimateSats !== null ? (
            <>
              <Text style={styles.feeText}>Network fee: ~{feeEstimateSats.toString()} sats</Text>
              <Text style={styles.totalText}>Total: {(effectiveAmountSats + feeEstimateSats).toString()} sats</Text>
            </>
          ) : (
            feeError && <Text style={styles.hint}>{feeError}</Text>
          )}
        </View>
      )}

      <TouchableOpacity
        style={[styles.primaryButton, (!invoice.trim() || effectiveAmountSats === null) && styles.primaryButtonDisabled]}
        onPress={pay}
        disabled={!invoice.trim() || effectiveAmountSats === null}
      >
        <Text style={styles.primaryButtonText}>Pay</Text>
      </TouchableOpacity>

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
    fontSize: 15,
    fontFamily: "monospace",
    minHeight: 90,
    textAlignVertical: "top",
    marginBottom: spacing.md,
  },
  amountInput: {
    backgroundColor: colors.surface,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radii.md,
    padding: spacing.md,
    color: colors.textPrimary,
    fontSize: 17,
    fontWeight: "700",
  },
  amountInputLocked: {
    color: colors.textSecondary,
  },
  hint: {
    color: colors.textMuted,
    fontSize: 12,
    marginTop: spacing.xs,
  },
  feeBlock: {
    marginTop: spacing.md,
    paddingTop: spacing.sm,
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: colors.border,
  },
  feeText: {
    color: colors.textSecondary,
    fontSize: 13,
  },
  totalText: {
    color: colors.textPrimary,
    fontSize: 15,
    fontWeight: "700",
    marginTop: 2,
  },
  primaryButton: {
    backgroundColor: colors.accent,
    borderRadius: radii.pill,
    paddingVertical: spacing.md,
    alignItems: "center",
    marginTop: spacing.md,
  },
  primaryButtonDisabled: {
    opacity: 0.5,
  },
  primaryButtonText: {
    color: colors.accentText,
    fontSize: 17,
    fontWeight: "700",
  },
  secondaryButtonOutline: {
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radii.pill,
    paddingVertical: spacing.md,
    alignItems: "center",
    marginBottom: spacing.lg,
  },
  secondaryButtonOutlineText: {
    color: colors.textPrimary,
    fontSize: 16,
    fontWeight: "600",
  },
  secondaryButtonText: {
    color: colors.textPrimary,
    fontSize: 15,
  },
  error: {
    color: colors.error,
    marginTop: spacing.lg,
    textAlign: "center",
  },
  scannerContainer: {
    flex: 1,
    backgroundColor: "#000",
  },
  scannerOverlay: {
    flex: 1,
    alignItems: "center",
    justifyContent: "center",
    backgroundColor: "rgba(0,0,0,0.15)",
  },
  scannerFrame: {
    width: 250,
    height: 250,
    borderWidth: 2,
    borderColor: colors.accent,
    borderRadius: radii.md,
  },
  cancelScanButton: {
    position: "absolute",
    bottom: 60,
    backgroundColor: colors.surfaceElevated,
    paddingHorizontal: spacing.xl,
    paddingVertical: spacing.md,
    borderRadius: radii.pill,
  },
});
