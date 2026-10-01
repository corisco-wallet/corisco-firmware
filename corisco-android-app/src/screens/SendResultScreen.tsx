// Full-screen outcome shown once a payment attempt finishes (success or
// failure) -- replaces the old behavior of just falling back to the Send
// form with a small text line at the bottom, which read as "nothing
// happened" rather than a clear result, especially right after the
// full-screen SendingScreen spinner.

import { StyleSheet, Text, TouchableOpacity, View } from "react-native";
import Svg, { Path, Polyline } from "react-native-svg";
import { colors, radii, spacing } from "../theme";

function CheckIcon({ color, size = 40 }: { color: string; size?: number }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
      <Polyline points="20 6 9 17 4 12" />
    </Svg>
  );
}

function XIcon({ color, size = 40 }: { color: string; size?: number }) {
  return (
    <Svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
      <Path d="M18 6L6 18M6 6l12 12" />
    </Svg>
  );
}

export function SendResultScreen({
  ok,
  message,
  onDone,
  onRetry,
}: {
  ok: boolean;
  message: string;
  onDone: () => void;
  /** Only offered on failure -- keeps the invoice/amount already entered
   * instead of sending the user all the way back to a blank form. */
  onRetry?: () => void;
}) {
  return (
    <View style={styles.container}>
      <Text style={styles.brandText}>Corisco</Text>
      <View style={[styles.iconWrap, ok ? styles.iconWrapSuccess : styles.iconWrapError]}>
        {ok ? <CheckIcon color={colors.success} /> : <XIcon color={colors.error} />}
      </View>
      <Text style={[styles.title, ok ? styles.titleSuccess : styles.titleError]}>
        {ok ? "Payment sent" : "Payment failed"}
      </Text>
      <Text style={styles.message} numberOfLines={4}>
        {message}
      </Text>
      <View style={styles.actions}>
        {!ok && onRetry && (
          <TouchableOpacity style={styles.secondaryButtonOutline} onPress={onRetry}>
            <Text style={styles.secondaryButtonOutlineText}>Try again</Text>
          </TouchableOpacity>
        )}
        <TouchableOpacity style={styles.primaryButton} onPress={onDone}>
          <Text style={styles.primaryButtonText}>Done</Text>
        </TouchableOpacity>
      </View>
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
  brandText: {
    color: colors.accent,
    fontSize: 22,
    fontWeight: "700",
    letterSpacing: 1,
    marginBottom: spacing.xl,
  },
  iconWrap: {
    width: 72,
    height: 72,
    borderRadius: radii.pill,
    alignItems: "center",
    justifyContent: "center",
    marginBottom: spacing.lg,
  },
  iconWrapSuccess: {
    backgroundColor: "rgba(74, 222, 128, 0.15)",
  },
  iconWrapError: {
    backgroundColor: "rgba(255, 107, 107, 0.15)",
  },
  title: {
    fontSize: 22,
    fontWeight: "700",
    marginBottom: spacing.sm,
  },
  titleSuccess: {
    color: colors.success,
  },
  titleError: {
    color: colors.error,
  },
  message: {
    color: colors.textSecondary,
    fontSize: 14,
    textAlign: "center",
    marginBottom: spacing.xl,
  },
  actions: {
    flexDirection: "row",
    gap: spacing.md,
    alignSelf: "stretch",
  },
  primaryButton: {
    flex: 1,
    backgroundColor: colors.accent,
    borderRadius: radii.pill,
    paddingVertical: spacing.md,
    alignItems: "center",
  },
  primaryButtonText: {
    color: colors.accentText,
    fontSize: 16,
    fontWeight: "700",
  },
  secondaryButtonOutline: {
    flex: 1,
    borderWidth: 1,
    borderColor: colors.border,
    borderRadius: radii.pill,
    paddingVertical: spacing.md,
    alignItems: "center",
  },
  secondaryButtonOutlineText: {
    color: colors.textPrimary,
    fontSize: 16,
    fontWeight: "600",
  },
});
