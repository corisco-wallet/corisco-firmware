// Full-screen status shown while a payment is in flight -- same visual
// language as SyncingScreen's ring (brand text, dark background, accent
// color), but indeterminate rather than percentage-driven: unlike
// connectAndInit's several discrete, awaited stages, `payLightningInvoice`
// is one opaque call with no real sub-progress to report (it may also be
// waiting on a physical tap on the hardware signer's confirm screen, an
// unknown/human-paced duration) -- a fake percentage here would violate
// the same "real progress only" principle SyncingScreen itself documents.
// A continuously-spinning partial ring is the honest equivalent: "working,
// no known ETA," not "72% done."

import { useEffect, useRef } from "react";
import { Animated, Easing, StyleSheet, Text, View } from "react-native";
import Svg, { Circle } from "react-native-svg";
import { colors, spacing } from "../theme";

const SIZE = 96;
const STROKE_WIDTH = 8;
const RADIUS = (SIZE - STROKE_WIDTH) / 2;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;
// A fixed ~30% arc, continuously rotated -- reads as "spinning," not "30%
// complete" (there's no percent label at all, unlike SyncingScreen).
const ARC_FRACTION = 0.3;

export function SendingScreen({ label }: { label: string }) {
  const rotation = useRef(new Animated.Value(0)).current;

  useEffect(() => {
    const loop = Animated.loop(
      Animated.timing(rotation, {
        toValue: 1,
        duration: 1100,
        easing: Easing.linear,
        useNativeDriver: true,
      }),
    );
    loop.start();
    return () => loop.stop();
  }, [rotation]);

  const spin = rotation.interpolate({ inputRange: [0, 1], outputRange: ["0deg", "360deg"] });

  return (
    <View style={styles.container}>
      <Text style={styles.brandText}>Corisco</Text>
      <Animated.View style={[styles.ring, { transform: [{ rotate: spin }] }]}>
        <Svg width={SIZE} height={SIZE}>
          <Circle cx={SIZE / 2} cy={SIZE / 2} r={RADIUS} stroke={colors.border} strokeWidth={STROKE_WIDTH} fill="none" />
          <Circle
            cx={SIZE / 2}
            cy={SIZE / 2}
            r={RADIUS}
            stroke={colors.accent}
            strokeWidth={STROKE_WIDTH}
            fill="none"
            strokeDasharray={`${CIRCUMFERENCE * ARC_FRACTION}, ${CIRCUMFERENCE}`}
            strokeLinecap="round"
          />
        </Svg>
      </Animated.View>
      <Text style={styles.label}>{label}</Text>
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
  ring: {
    width: SIZE,
    height: SIZE,
  },
  label: {
    marginTop: spacing.lg,
    color: colors.textSecondary,
    fontSize: 14,
    textAlign: "center",
  },
});
