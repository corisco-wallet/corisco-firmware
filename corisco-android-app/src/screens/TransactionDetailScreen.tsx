// Full detail for one transfer, opened by tapping a row in ActivityList.
// Same amount formatting (unit/currency/hide) as Home, so a number here
// always reads the same way as the one that led here.

import { ScrollView, StyleSheet, Text, TouchableOpacity, View } from "react-native";
import type { WalletTransfer } from "@buildonspark/spark-sdk/types";
import { colors, radii, spacing } from "../theme";
import { formatBtc, formatFiat, satsToFiat } from "../price";
import type { Settings } from "../settings-store";
import { TYPE_LABELS } from "../components/ActivityList";

const HIDDEN = "••••••";

function shortHex(value: string, head = 10, tail = 8): string {
  return value.length <= head + tail + 1 ? value : `${value.slice(0, head)}…${value.slice(-tail)}`;
}

function formatDateTime(date: Date | undefined): string {
  if (!date) return "Unknown";
  return date.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function humanizeStatus(status: string): string {
  const words = status.toLowerCase().replace(/_/g, " ").split(" ");
  return words.map((w) => w.charAt(0).toUpperCase() + w.slice(1)).join(" ");
}

function DetailRow({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <View style={styles.detailRow}>
      <Text style={styles.detailLabel}>{label}</Text>
      <Text style={[styles.detailValue, mono && styles.detailValueMono]} numberOfLines={1} ellipsizeMode="middle">
        {value}
      </Text>
    </View>
  );
}

export function TransactionDetailScreen({
  transfer,
  settings,
  btcPrice,
  onBack,
}: {
  transfer: WalletTransfer;
  settings: Settings;
  btcPrice: number | null;
  onBack: () => void;
}) {
  const incoming = transfer.transferDirection === "INCOMING";
  const sats = BigInt(Math.trunc(transfer.totalValue));
  const amountText = settings.balanceUnit === "btc" ? formatBtc(sats) : sats.toString();
  const fiatText = btcPrice !== null ? formatFiat(satsToFiat(sats, btcPrice), settings.currency) : null;
  const label = TYPE_LABELS[transfer.type] ?? transfer.type;
  const pending = transfer.status !== "TRANSFER_STATUS_COMPLETED";

  return (
    <ScrollView contentContainerStyle={styles.container}>
      <TouchableOpacity onPress={onBack} style={styles.backButton}>
        <Text style={styles.backText}>{"< Back"}</Text>
      </TouchableOpacity>

      <View style={styles.hero}>
        <View style={[styles.iconWrap, incoming ? styles.iconWrapIn : styles.iconWrapOut]}>
          <Text style={[styles.icon, incoming ? styles.iconIn : styles.iconOut]}>{incoming ? "↓" : "↑"}</Text>
        </View>
        <Text style={[styles.amount, incoming ? styles.amountIn : styles.amountOut]}>
          {incoming ? "+" : "-"}
          {settings.hideAmounts ? HIDDEN : amountText} {settings.balanceUnit === "btc" ? "BTC" : "sats"}
        </Text>
        {fiatText && <Text style={styles.fiat}>{settings.hideAmounts ? HIDDEN : fiatText}</Text>}
        <View style={[styles.statusBadge, pending ? styles.statusBadgePending : styles.statusBadgeDone]}>
          <Text style={[styles.statusText, pending ? styles.statusTextPending : styles.statusTextDone]}>
            {pending ? "Pending" : "Completed"}
          </Text>
        </View>
      </View>

      <View style={styles.detailsCard}>
        <DetailRow label="Type" value={label} />
        <DetailRow label="Date" value={formatDateTime(transfer.createdTime)} />
        <DetailRow label="Status" value={humanizeStatus(transfer.status)} />
        {incoming ? (
          <DetailRow label="From" value={shortHex(transfer.senderIdentityPublicKey)} mono />
        ) : (
          <DetailRow label="To" value={shortHex(transfer.receiverIdentityPublicKey)} mono />
        )}
        <DetailRow label="Transfer ID" value={shortHex(transfer.id)} mono />
        {transfer.leaves.length > 0 && <DetailRow label="Leaves" value={String(transfer.leaves.length)} />}
        {transfer.sparkInvoice && <DetailRow label="Spark invoice" value={shortHex(transfer.sparkInvoice)} mono />}
      </View>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  container: {
    flexGrow: 1,
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
  hero: {
    alignItems: "center",
    marginBottom: spacing.xl,
  },
  iconWrap: {
    width: 48,
    height: 48,
    borderRadius: radii.pill,
    alignItems: "center",
    justifyContent: "center",
    marginBottom: spacing.md,
  },
  iconWrapIn: {
    backgroundColor: "rgba(74, 222, 128, 0.15)",
  },
  iconWrapOut: {
    backgroundColor: "rgba(245, 185, 66, 0.15)",
  },
  icon: {
    fontSize: 22,
    fontWeight: "700",
  },
  iconIn: {
    color: colors.success,
  },
  iconOut: {
    color: colors.accent,
  },
  amount: {
    fontSize: 30,
    fontWeight: "800",
    letterSpacing: -0.5,
  },
  amountIn: {
    color: colors.success,
  },
  amountOut: {
    color: colors.textPrimary,
  },
  fiat: {
    color: colors.textSecondary,
    fontSize: 15,
    fontWeight: "600",
    marginTop: spacing.xs,
  },
  statusBadge: {
    marginTop: spacing.md,
    paddingVertical: spacing.xs,
    paddingHorizontal: spacing.md,
    borderRadius: radii.pill,
  },
  statusBadgeDone: {
    backgroundColor: "rgba(74, 222, 128, 0.15)",
  },
  statusBadgePending: {
    backgroundColor: "rgba(245, 185, 66, 0.15)",
  },
  statusText: {
    fontSize: 12,
    fontWeight: "700",
  },
  statusTextDone: {
    color: colors.success,
  },
  statusTextPending: {
    color: colors.accent,
  },
  detailsCard: {
    backgroundColor: colors.surface,
    borderRadius: radii.md,
    borderWidth: 1,
    borderColor: colors.border,
    overflow: "hidden",
  },
  detailRow: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    paddingVertical: spacing.md,
    paddingHorizontal: spacing.md,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: colors.border,
    gap: spacing.md,
  },
  detailLabel: {
    color: colors.textMuted,
    fontSize: 13,
  },
  detailValue: {
    color: colors.textPrimary,
    fontSize: 13,
    fontWeight: "600",
    flexShrink: 1,
    textAlign: "right",
  },
  detailValueMono: {
    fontFamily: "monospace",
  },
});
