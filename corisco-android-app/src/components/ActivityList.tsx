// Last N transfers, newest first -- a thin display layer over
// `wallet.getTransfers()`. Direction/amount sign only (INCOMING = credit,
// OUTGOING = debit); no private key material involved in reading this,
// it's just an authenticated query.

import { StyleSheet, Text, TouchableOpacity, View } from "react-native";
import type { WalletTransfer } from "@buildonspark/spark-sdk/types";
import { colors, radii, spacing } from "../theme";

// Shared with TransactionDetailScreen.tsx, which shows the full name for
// whichever type this maps -- kept here since this is where the type ->
// label mapping already lived.
export const TYPE_LABELS: Partial<Record<WalletTransfer["type"], string>> = {
  PREIMAGE_SWAP: "Lightning",
  TRANSFER: "Spark transfer",
  COOPERATIVE_EXIT: "On-chain withdrawal",
  UTXO_SWAP: "On-chain deposit",
};

function formatWhen(date: Date | undefined): string {
  if (!date) return "";
  const diffMs = Date.now() - date.getTime();
  const diffMin = Math.floor(diffMs / 60_000);
  if (diffMin < 1) return "just now";
  if (diffMin < 60) return `${diffMin}m ago`;
  const diffHr = Math.floor(diffMin / 60);
  if (diffHr < 24) return `${diffHr}h ago`;
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

function Row({ transfer, onPress }: { transfer: WalletTransfer; onPress: () => void }) {
  const incoming = transfer.transferDirection === "INCOMING";
  const label = TYPE_LABELS[transfer.type] ?? transfer.type;
  const pending = transfer.status !== "TRANSFER_STATUS_COMPLETED";

  return (
    <TouchableOpacity style={styles.row} onPress={onPress}>
      <View style={[styles.iconWrap, incoming ? styles.iconWrapIn : styles.iconWrapOut]}>
        <Text style={[styles.icon, incoming ? styles.iconIn : styles.iconOut]}>{incoming ? "↓" : "↑"}</Text>
      </View>
      <View style={styles.middle}>
        <Text style={styles.label}>{label}</Text>
        <Text style={styles.when}>
          {formatWhen(transfer.createdTime)}
          {pending ? " · pending" : ""}
        </Text>
      </View>
      <Text style={[styles.amount, incoming ? styles.amountIn : styles.amountOut]}>
        {incoming ? "+" : "-"}
        {transfer.totalValue}
      </Text>
    </TouchableOpacity>
  );
}

export function ActivityList({
  transfers,
  loading,
  onSelect,
}: {
  transfers: WalletTransfer[];
  loading: boolean;
  onSelect: (transfer: WalletTransfer) => void;
}) {
  return (
    <View style={styles.container}>
      <Text style={styles.heading}>Recent activity</Text>
      {loading && transfers.length === 0 ? (
        <Text style={styles.empty}>Loading...</Text>
      ) : transfers.length === 0 ? (
        <Text style={styles.empty}>No transactions yet</Text>
      ) : (
        <View style={styles.list}>
          {transfers.map((t) => (
            <Row key={t.id} transfer={t} onPress={() => onSelect(t)} />
          ))}
        </View>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    marginTop: spacing.lg,
  },
  heading: {
    color: colors.textSecondary,
    fontSize: 13,
    fontWeight: "600",
    letterSpacing: 1,
    textTransform: "uppercase",
    marginBottom: spacing.sm,
  },
  empty: {
    color: colors.textMuted,
    fontSize: 14,
    paddingVertical: spacing.md,
  },
  list: {
    backgroundColor: colors.surface,
    borderRadius: radii.md,
    borderWidth: 1,
    borderColor: colors.border,
    overflow: "hidden",
  },
  row: {
    flexDirection: "row",
    alignItems: "center",
    paddingVertical: spacing.md,
    paddingHorizontal: spacing.md,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: colors.border,
  },
  iconWrap: {
    width: 32,
    height: 32,
    borderRadius: radii.pill,
    alignItems: "center",
    justifyContent: "center",
    marginRight: spacing.md,
  },
  iconWrapIn: {
    backgroundColor: "rgba(74, 222, 128, 0.15)",
  },
  iconWrapOut: {
    backgroundColor: "rgba(245, 185, 66, 0.15)",
  },
  icon: {
    fontSize: 16,
    fontWeight: "700",
  },
  iconIn: {
    color: colors.success,
  },
  iconOut: {
    color: colors.accent,
  },
  middle: {
    flex: 1,
  },
  label: {
    color: colors.textPrimary,
    fontSize: 15,
    fontWeight: "600",
  },
  when: {
    color: colors.textMuted,
    fontSize: 12,
    marginTop: 2,
  },
  amount: {
    fontSize: 15,
    fontWeight: "700",
    fontVariant: ["tabular-nums"],
  },
  amountIn: {
    color: colors.success,
  },
  amountOut: {
    color: colors.textPrimary,
  },
});
