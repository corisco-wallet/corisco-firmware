// Display preferences (currency, balance unit, privacy) plus the
// disconnect action -- moved here from the Home screen's footer so Home
// stays focused on the balance/activity, and destructive-ish actions live
// somewhere a stray tap can't reach them.

import { Alert, ScrollView, StyleSheet, Switch, Text, TouchableOpacity, View } from "react-native";
import { colors, radii, spacing } from "../theme";
import { CURRENCIES, TRANSACTION_COUNT_OPTIONS, type BalanceUnit, type Settings } from "../settings-store";

export function SettingsScreen({
  settings,
  onChange,
  onDisconnect,
  onBack,
}: {
  settings: Settings;
  onChange: (patch: Partial<Settings>) => void;
  onDisconnect: () => void;
  onBack: () => void;
}) {
  return (
    <ScrollView contentContainerStyle={styles.container}>
      <TouchableOpacity onPress={onBack} style={styles.backButton}>
        <Text style={styles.backText}>{"< Back"}</Text>
      </TouchableOpacity>

      <Text style={styles.title}>Settings</Text>

      <Text style={styles.sectionLabel}>Balance unit</Text>
      <View style={styles.segmented}>
        {(["sats", "btc"] as BalanceUnit[]).map((unit) => (
          <TouchableOpacity
            key={unit}
            style={[styles.segment, settings.balanceUnit === unit && styles.segmentActive]}
            onPress={() => onChange({ balanceUnit: unit })}
          >
            <Text style={[styles.segmentText, settings.balanceUnit === unit && styles.segmentTextActive]}>
              {unit === "sats" ? "Sats" : "BTC"}
            </Text>
          </TouchableOpacity>
        ))}
      </View>

      <Text style={styles.sectionLabel}>Currency</Text>
      <Text style={styles.sectionHint}>Used for the conversion shown under your balance.</Text>
      <View style={styles.currencyGrid}>
        {CURRENCIES.map((c) => (
          <TouchableOpacity
            key={c.code}
            style={[styles.currencyChip, settings.currency === c.code && styles.currencyChipActive]}
            onPress={() => onChange({ currency: c.code })}
          >
            <Text style={[styles.currencyChipText, settings.currency === c.code && styles.currencyChipTextActive]}>
              {c.label}
            </Text>
          </TouchableOpacity>
        ))}
      </View>

      <View style={styles.row}>
        <View style={styles.rowText}>
          <Text style={styles.rowTitle}>Hide amounts</Text>
          <Text style={styles.rowHint}>Masks balances on screen -- useful with others around.</Text>
        </View>
        <Switch
          value={settings.hideAmounts}
          onValueChange={(value) => onChange({ hideAmounts: value })}
          trackColor={{ false: colors.border, true: colors.accent }}
          thumbColor={colors.textPrimary}
        />
      </View>

      <Text style={styles.sectionLabel}>Transaction history</Text>
      <Text style={styles.sectionHint}>How many recent transactions to show on the home screen.</Text>
      <View style={styles.currencyGrid}>
        {TRANSACTION_COUNT_OPTIONS.map((n) => (
          <TouchableOpacity
            key={n}
            style={[styles.currencyChip, settings.showLastXTransactions === n && styles.currencyChipActive]}
            onPress={() => onChange({ showLastXTransactions: n })}
          >
            <Text
              style={[
                styles.currencyChipText,
                settings.showLastXTransactions === n && styles.currencyChipTextActive,
              ]}
            >
              {n}
            </Text>
          </TouchableOpacity>
        ))}
      </View>

      <View style={styles.divider} />

      <TouchableOpacity
        style={styles.disconnectButton}
        onPress={() =>
          Alert.alert("Disconnect signer?", "You can reconnect to it anytime from the signer list.", [
            { text: "Cancel", style: "cancel" },
            { text: "Disconnect", style: "destructive", onPress: onDisconnect },
          ])
        }
      >
        <Text style={styles.disconnectButtonText}>Disconnect signer</Text>
      </TouchableOpacity>
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
  title: {
    color: colors.textPrimary,
    fontSize: 28,
    fontWeight: "700",
    marginBottom: spacing.xl,
  },
  sectionLabel: {
    color: colors.textPrimary,
    fontSize: 15,
    fontWeight: "700",
    marginBottom: spacing.xs,
  },
  sectionHint: {
    color: colors.textMuted,
    fontSize: 12,
    marginBottom: spacing.sm,
  },
  segmented: {
    flexDirection: "row",
    backgroundColor: colors.surface,
    borderRadius: radii.pill,
    borderWidth: 1,
    borderColor: colors.border,
    padding: 4,
    marginBottom: spacing.xl,
  },
  segment: {
    flex: 1,
    paddingVertical: spacing.sm,
    borderRadius: radii.pill,
    alignItems: "center",
  },
  segmentActive: {
    backgroundColor: colors.accent,
  },
  segmentText: {
    color: colors.textSecondary,
    fontSize: 14,
    fontWeight: "600",
  },
  segmentTextActive: {
    color: colors.accentText,
  },
  currencyGrid: {
    flexDirection: "row",
    flexWrap: "wrap",
    gap: spacing.sm,
    marginBottom: spacing.xl,
  },
  currencyChip: {
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.md,
    borderRadius: radii.pill,
    borderWidth: 1,
    borderColor: colors.border,
    backgroundColor: colors.surface,
  },
  currencyChipActive: {
    backgroundColor: colors.accent,
    borderColor: colors.accent,
  },
  currencyChipText: {
    color: colors.textSecondary,
    fontSize: 13,
    fontWeight: "600",
  },
  currencyChipTextActive: {
    color: colors.accentText,
  },
  row: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    paddingVertical: spacing.md,
  },
  rowText: {
    flex: 1,
    marginRight: spacing.md,
  },
  rowTitle: {
    color: colors.textPrimary,
    fontSize: 15,
    fontWeight: "600",
  },
  rowHint: {
    color: colors.textMuted,
    fontSize: 12,
    marginTop: 2,
  },
  divider: {
    height: 1,
    backgroundColor: colors.border,
    marginVertical: spacing.xl,
  },
  disconnectButton: {
    alignItems: "center",
    paddingVertical: spacing.md,
  },
  disconnectButtonText: {
    color: colors.textMuted,
    fontSize: 12,
    fontWeight: "600",
  },
});
