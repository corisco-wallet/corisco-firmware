// Shown the first time a signer is paired (and any time the user asks to
// pair another one): scans for nearby devices and lists them, rather than
// silently connecting to whichever one is found first. Today every real
// signer advertises the same fixed name (`SparkHW` -- see ble-transport.ts's
// `scanForCandidates` doc comment), so this list can only ever show one
// distinct entry in practice; the picker exists now so that a future
// firmware change giving each device its own default name makes multiple
// nearby signers distinguishable with no app change beyond that.

import { ActivityIndicator, FlatList, StyleSheet, Text, TouchableOpacity, View } from "react-native";
import { colors, radii, spacing } from "../theme";
import type { ScanResult } from "../ble-transport";

export function PairScanScreen({
  scanning,
  candidates,
  connecting,
  connectingId,
  connectStatus,
  error,
  onSelect,
  onRescan,
  onCancel,
}: {
  scanning: boolean;
  candidates: ScanResult[];
  connecting: boolean;
  connectingId: string | null;
  connectStatus: string;
  error: string | null;
  onSelect: (candidate: ScanResult) => void;
  onRescan: () => void;
  onCancel: () => void;
}) {
  return (
    <View style={styles.container}>
      <Text style={styles.brandText}>Corisco</Text>
      <Text style={styles.title}>Pair a signer</Text>

      {scanning && (
        <View style={styles.statusRow}>
          <ActivityIndicator color={colors.accent} size="small" />
          <Text style={styles.statusText}>Scanning for nearby signers...</Text>
        </View>
      )}
      {error && <Text style={styles.errorBody}>{error}</Text>}
      {!scanning && candidates.length === 0 && !error && (
        <Text style={styles.instructionText}>No signers found nearby. Make sure it's powered on and advertising.</Text>
      )}

      <FlatList
        style={styles.list}
        data={candidates}
        keyExtractor={(c) => c.id}
        renderItem={({ item }) => {
          const isConnectingThis = connecting && connectingId === item.id;
          return (
            <TouchableOpacity style={styles.row} disabled={connecting} onPress={() => onSelect(item)}>
              <View style={styles.rowText}>
                <Text style={styles.rowName}>{item.name}</Text>
                <Text style={styles.rowId}>{item.id}</Text>
              </View>
              {isConnectingThis ? (
                <ActivityIndicator color={colors.accent} size="small" />
              ) : (
                <Text style={styles.rowAction}>Pair</Text>
              )}
            </TouchableOpacity>
          );
        }}
      />

      {connecting && connectingId && <Text style={styles.statusText}>{connectStatus}</Text>}

      <View style={styles.actions}>
        <TouchableOpacity style={styles.secondaryButton} disabled={connecting} onPress={onCancel}>
          <Text style={styles.secondaryButtonText}>Cancel</Text>
        </TouchableOpacity>
        <TouchableOpacity style={styles.secondaryButton} disabled={connecting || scanning} onPress={onRescan}>
          <Text style={styles.secondaryButtonText}>{scanning ? "Scanning..." : "Scan again"}</Text>
        </TouchableOpacity>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: colors.background,
    padding: spacing.lg,
    paddingTop: 72,
  },
  brandText: {
    color: colors.accent,
    fontSize: 22,
    fontWeight: "700",
    letterSpacing: 1,
    textAlign: "center",
    marginBottom: spacing.sm,
  },
  title: {
    color: colors.textPrimary,
    fontSize: 16,
    fontWeight: "600",
    textAlign: "center",
    marginBottom: spacing.md,
  },
  statusRow: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "center",
    gap: spacing.sm,
    marginBottom: spacing.md,
  },
  statusText: {
    color: colors.textSecondary,
    fontSize: 13,
    textAlign: "center",
  },
  instructionText: {
    color: colors.textSecondary,
    fontSize: 13,
    textAlign: "center",
    marginBottom: spacing.md,
  },
  errorBody: {
    color: colors.error,
    fontSize: 13,
    textAlign: "center",
    marginBottom: spacing.md,
  },
  list: {
    flexGrow: 0,
    marginBottom: spacing.md,
  },
  row: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    backgroundColor: colors.surface,
    borderRadius: radii.md,
    borderWidth: 1,
    borderColor: colors.border,
    paddingVertical: spacing.md,
    paddingHorizontal: spacing.md,
    marginBottom: spacing.sm,
  },
  rowText: {
    flexShrink: 1,
  },
  rowName: {
    color: colors.textPrimary,
    fontSize: 16,
    fontWeight: "600",
  },
  rowId: {
    color: colors.textMuted,
    fontSize: 11,
    fontFamily: "monospace",
    marginTop: 2,
  },
  rowAction: {
    color: colors.accent,
    fontSize: 13,
    fontWeight: "700",
    marginLeft: spacing.sm,
  },
  actions: {
    flexDirection: "row",
    gap: spacing.md,
    marginTop: "auto",
  },
  secondaryButton: {
    flex: 1,
    borderRadius: radii.pill,
    borderWidth: 1,
    borderColor: colors.border,
    paddingVertical: spacing.md,
    alignItems: "center",
  },
  secondaryButtonText: {
    color: colors.textSecondary,
    fontSize: 14,
    fontWeight: "600",
  },
});
