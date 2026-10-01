// Balance front and center, with Receive/Send actions below. No private
// key material ever touches this screen (or this app).

import { ActivityIndicator, RefreshControl, ScrollView, StyleSheet, Text, TouchableOpacity, View } from "react-native";
import type { WalletTransfer } from "@buildonspark/spark-sdk/types";
import { colors, radii, spacing } from "../theme";
import { ActivityList } from "../components/ActivityList";
import { formatBtc, formatFiat, satsToFiat } from "../price";
import type { Settings } from "../settings-store";

const HIDDEN = "••••••";

export function HomeScreen({
  identityPubkey,
  availableSats,
  incomingSats,
  claiming,
  loading,
  refreshing,
  onRefresh,
  onReceive,
  onSend,
  onSettings,
  onSelectTransfer,
  transfers,
  transfersLoading,
  settings,
  btcPrice,
}: {
  identityPubkey: string | null;
  availableSats: bigint | null;
  // Sats the server already knows about but that aren't claimed/spendable
  // yet (a pending Lightning receive still working through its claim
  // steps), and whether a claim pass is actively in flight right now --
  // together these are what let a climbing balance read as "still
  // syncing" instead of a mysteriously wrong number.
  incomingSats: bigint | null;
  claiming: boolean;
  loading: boolean;
  refreshing: boolean;
  onRefresh: () => void;
  onReceive: () => void;
  onSend: () => void;
  onSettings: () => void;
  onSelectTransfer: (transfer: WalletTransfer) => void;
  transfers: WalletTransfer[];
  transfersLoading: boolean;
  settings: Settings;
  // Price of 1 BTC in `settings.currency`, or `null` if it hasn't loaded
  // (offline, still fetching, or the fetch failed) -- in which case the
  // conversion line under the balance is just omitted.
  btcPrice: number | null;
}) {
  const sats = availableSats ?? 0n;
  const balanceText = settings.balanceUnit === "btc" ? formatBtc(sats) : sats.toString();
  const fiatText = btcPrice !== null ? formatFiat(satsToFiat(sats, btcPrice), settings.currency) : null;
  return (
    <ScrollView
      contentContainerStyle={styles.container}
      refreshControl={<RefreshControl tintColor={colors.accent} refreshing={refreshing} onRefresh={onRefresh} />}
    >
      <View style={styles.header}>
        <View style={styles.headerSideSpacer} />
        <Text style={styles.brandText}>Corisco</Text>
        <TouchableOpacity onPress={onSettings} style={styles.settingsButton} hitSlop={8}>
          <Text style={styles.settingsIcon}>⚙</Text>
        </TouchableOpacity>
      </View>

      <View style={styles.balanceBlock}>
        {loading && availableSats === null ? (
          <ActivityIndicator color={colors.accent} size="large" />
        ) : (
          <>
            <Text style={styles.balanceAmount}>{settings.hideAmounts ? HIDDEN : balanceText}</Text>
            <Text style={styles.balanceUnit}>{settings.balanceUnit === "btc" ? "BTC" : "sats"}</Text>
            {fiatText && (
              <Text style={styles.fiatText}>{settings.hideAmounts ? HIDDEN : fiatText}</Text>
            )}
          </>
        )}
        {claiming ? (
          <View style={styles.pendingRow}>
            <ActivityIndicator color={colors.textSecondary} size="small" />
            <Text style={styles.pendingText}>Checking for pending payments...</Text>
          </View>
        ) : (
          incomingSats !== null &&
          incomingSats > 0n && (
            <View style={styles.pendingRow}>
              <Text style={styles.pendingText}>
                +{settings.hideAmounts ? HIDDEN : incomingSats.toString()} sats incoming
              </Text>
            </View>
          )
        )}

        {/* No more background poll -- checking for new payments is a real
            BLE + network round-trip, not a free status check, so it only
            happens when asked: this button, or the pull-to-refresh above. */}
        <TouchableOpacity
          style={[styles.refreshButton, refreshing && styles.refreshButtonDisabled]}
          onPress={onRefresh}
          disabled={refreshing}
        >
          {refreshing ? (
            <ActivityIndicator color={colors.textSecondary} size="small" />
          ) : (
            <Text style={styles.refreshButtonText}>Refresh</Text>
          )}
        </TouchableOpacity>
      </View>

      <View style={styles.actions}>
        <TouchableOpacity style={styles.actionButton} onPress={onReceive}>
          <Text style={styles.actionButtonText}>Receive</Text>
        </TouchableOpacity>
        <TouchableOpacity style={[styles.actionButton, styles.actionButtonOutline]} onPress={onSend}>
          <Text style={[styles.actionButtonText, styles.actionButtonOutlineText]}>Send</Text>
        </TouchableOpacity>
      </View>

      <ActivityList transfers={transfers} loading={transfersLoading} onSelect={onSelectTransfer} />

      {identityPubkey && (
        <Text style={styles.pubkey} numberOfLines={1} ellipsizeMode="middle">
          {identityPubkey}
        </Text>
      )}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  container: {
    flexGrow: 1,
    backgroundColor: colors.background,
    paddingTop: 60,
    paddingHorizontal: spacing.lg,
  },
  header: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    marginBottom: spacing.xl,
  },
  headerSideSpacer: {
    width: 28,
  },
  brandText: {
    color: colors.textSecondary,
    fontSize: 15,
    fontWeight: "600",
    letterSpacing: 3,
    textTransform: "uppercase",
  },
  settingsButton: {
    width: 28,
    alignItems: "flex-end",
  },
  settingsIcon: {
    color: colors.textSecondary,
    fontSize: 20,
  },
  balanceBlock: {
    alignItems: "center",
    justifyContent: "center",
    minHeight: 110,
    marginBottom: spacing.xl,
  },
  balanceAmount: {
    color: colors.textPrimary,
    fontSize: 56,
    fontWeight: "800",
    letterSpacing: -1,
  },
  fiatText: {
    color: colors.textSecondary,
    fontSize: 15,
    fontWeight: "600",
    marginTop: spacing.xs,
  },
  balanceUnit: {
    color: colors.accent,
    fontSize: 16,
    fontWeight: "700",
    letterSpacing: 2,
    textTransform: "uppercase",
    marginTop: -spacing.xs,
  },
  pendingRow: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.xs,
    marginTop: spacing.sm,
  },
  pendingText: {
    color: colors.textSecondary,
    fontSize: 13,
    fontWeight: "600",
  },
  refreshButton: {
    marginTop: spacing.md,
    paddingVertical: spacing.xs,
    paddingHorizontal: spacing.md,
    borderRadius: radii.pill,
    borderWidth: 1,
    borderColor: colors.border,
    minWidth: 84,
    alignItems: "center",
  },
  refreshButtonDisabled: {
    opacity: 0.5,
  },
  refreshButtonText: {
    color: colors.textSecondary,
    fontSize: 13,
    fontWeight: "600",
  },
  actions: {
    flexDirection: "row",
    gap: spacing.md,
    marginBottom: spacing.xl,
  },
  actionButton: {
    flex: 1,
    backgroundColor: colors.accent,
    borderRadius: radii.pill,
    paddingVertical: spacing.md,
    alignItems: "center",
  },
  actionButtonOutline: {
    backgroundColor: "transparent",
    borderWidth: 1,
    borderColor: colors.border,
  },
  actionButtonText: {
    color: colors.accentText,
    fontSize: 17,
    fontWeight: "700",
  },
  actionButtonOutlineText: {
    color: colors.textPrimary,
  },
  pubkey: {
    color: colors.textMuted,
    fontSize: 11,
    fontFamily: "monospace",
    textAlign: "center",
    marginTop: "auto",
    paddingTop: spacing.lg,
    paddingBottom: spacing.lg,
  },
});
