// Shown once, right after a brand-new signer finishes pairing -- lets the
// user give it a memorable name before it's saved (see device-store.ts) and
// shows up in the picker on future launches. `Alert.prompt` would do this
// in one line but is iOS-only; this app targets Android (see corisco-android-app's
// APK build), hence a real screen instead.

import { useState } from "react";
import { StyleSheet, Text, TextInput, TouchableOpacity, View } from "react-native";
import { colors, radii, spacing } from "../theme";

export function NameSignerScreen({
  defaultName,
  identityPubkey,
  onSave,
}: {
  defaultName: string;
  identityPubkey: string;
  onSave: (name: string) => void;
}) {
  const [name, setName] = useState(defaultName);

  return (
    <View style={styles.container}>
      <Text style={styles.title}>Signer paired</Text>
      <Text style={styles.subtitle}>Give this device a name so you can pick it from the list next time.</Text>
      <TextInput
        style={styles.input}
        value={name}
        onChangeText={setName}
        placeholder="e.g. Living room signer"
        placeholderTextColor={colors.textMuted}
        autoFocus
        maxLength={40}
      />
      <Text style={styles.pubkey} numberOfLines={1} ellipsizeMode="middle">
        {identityPubkey}
      </Text>
      <TouchableOpacity
        style={styles.saveButton}
        onPress={() => onSave(name.trim() === "" ? defaultName : name.trim())}
      >
        <Text style={styles.saveButtonText}>Continue</Text>
      </TouchableOpacity>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: colors.background,
    alignItems: "center",
    justifyContent: "center",
    padding: spacing.lg,
  },
  title: {
    color: colors.textPrimary,
    fontSize: 20,
    fontWeight: "700",
    marginBottom: spacing.sm,
  },
  subtitle: {
    color: colors.textSecondary,
    fontSize: 13,
    textAlign: "center",
    marginBottom: spacing.lg,
  },
  input: {
    width: "100%",
    backgroundColor: colors.surface,
    borderRadius: radii.md,
    borderWidth: 1,
    borderColor: colors.border,
    paddingHorizontal: spacing.md,
    paddingVertical: spacing.sm,
    color: colors.textPrimary,
    fontSize: 15,
    marginBottom: spacing.sm,
  },
  pubkey: {
    color: colors.textMuted,
    fontSize: 11,
    fontFamily: "monospace",
    textAlign: "center",
    marginBottom: spacing.lg,
  },
  saveButton: {
    backgroundColor: colors.accent,
    borderRadius: radii.pill,
    paddingVertical: spacing.md,
    paddingHorizontal: spacing.xl,
    alignSelf: "stretch",
    alignItems: "center",
  },
  saveButtonText: {
    color: colors.accentText,
    fontSize: 15,
    fontWeight: "700",
  },
});
