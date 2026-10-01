// First screen once at least one signer has been paired before: pick a
// saved device instead of re-scanning/re-pairing from scratch every launch
// (see device-store.ts). Long-press a row to forget it -- useful once a
// device's bond goes stale (re-flashed, factory-reset) and it would
// otherwise sit in the list as a dead entry.

import { Alert, FlatList, StyleSheet, Text, TouchableOpacity, View } from "react-native";
import { colors, radii, spacing } from "../theme";
import type { SavedDevice } from "../device-store";

function shortPubkey(pubkey: string): string {
  return pubkey.length <= 20 ? pubkey : `${pubkey.slice(0, 10)}…${pubkey.slice(-8)}`;
}

export function SignerListScreen({
  devices,
  connecting,
  connectingId,
  connectStatus,
  initError,
  onSelect,
  onForget,
  onPairNew,
}: {
  devices: SavedDevice[];
  connecting: boolean;
  connectingId: string | null;
  connectStatus: string;
  initError: string | null;
  onSelect: (device: SavedDevice) => void;
  onForget: (device: SavedDevice) => void;
  onPairNew: () => void;
}) {
  return (
    <View style={styles.container}>
      <Text style={styles.brandText}>Corisco</Text>
      <Text style={styles.instructionText}>Choose a signer to connect to.</Text>
      {initError && <Text style={styles.errorBody}>{initError}</Text>}

      <FlatList
        style={styles.list}
        data={devices}
        keyExtractor={(d) => d.id}
        renderItem={({ item }) => {
          const isConnectingThis = connecting && connectingId === item.id;
          return (
            <TouchableOpacity
              style={styles.row}
              disabled={connecting}
              onPress={() => onSelect(item)}
              onLongPress={() =>
                Alert.alert("Forget signer?", `Remove "${item.name}" from this list?`, [
                  { text: "Cancel", style: "cancel" },
                  { text: "Forget", style: "destructive", onPress: () => onForget(item) },
                ])
              }
            >
              <View style={styles.rowText}>
                <Text style={styles.rowName}>{item.name}</Text>
                <Text style={styles.rowPubkey}>{shortPubkey(item.identityPubkey)}</Text>
              </View>
              {isConnectingThis && <Text style={styles.rowStatus}>{connectStatus}</Text>}
            </TouchableOpacity>
          );
        }}
      />

      <TouchableOpacity style={styles.pairButton} disabled={connecting} onPress={onPairNew}>
        <Text style={styles.pairButtonText}>{connecting && !connectingId ? connectStatus : "+ Pair new signer"}</Text>
      </TouchableOpacity>
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
  instructionText: {
    color: colors.textSecondary,
    fontSize: 14,
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
    marginBottom: spacing.lg,
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
  rowPubkey: {
    color: colors.textMuted,
    fontSize: 11,
    fontFamily: "monospace",
    marginTop: 2,
  },
  rowStatus: {
    color: colors.accent,
    fontSize: 11,
    marginLeft: spacing.sm,
  },
  pairButton: {
    borderRadius: radii.pill,
    borderWidth: 1,
    borderColor: colors.border,
    paddingVertical: spacing.md,
    alignItems: "center",
  },
  pairButtonText: {
    color: colors.textSecondary,
    fontSize: 14,
    fontWeight: "600",
  },
});
