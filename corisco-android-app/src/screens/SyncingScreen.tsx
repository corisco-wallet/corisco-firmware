// Full-screen "getting your wallet ready" step between a successful BLE
// connection and the Home screen -- claiming pending transfers,
// consolidating leaves, and fetching the balance/transaction list are all
// real network + BLE round-trips (see App.tsx's `connectAndInit`), so
// jumping straight to Home would either show stale/zero numbers for a
// few seconds or need its own per-widget loading states scattered across
// the screen. One ring that fills as each stage finishes reads as a
// single, honest "here's how far along this is" instead.
//
// `progress` is driven by real stage completions in `connectAndInit`, not
// a fake timer -- 100% is only ever reached once the balance and transfer
// list are actually in hand, right before Home renders them.

import { useEffect, useRef } from "react";
import { Animated, StyleSheet, Text, View } from "react-native";
import Svg, { Circle } from "react-native-svg";
import { colors, spacing } from "../theme";

const SIZE = 132;
const STROKE_WIDTH = 8;
const RADIUS = (SIZE - STROKE_WIDTH) / 2;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

const AnimatedCircle = Animated.createAnimatedComponent(Circle);

export function SyncingScreen({ progress, label }: { progress: number; label: string }) {
  const animated = useRef(new Animated.Value(0)).current;

  useEffect(() => {
    Animated.timing(animated, {
      toValue: progress,
      duration: 350,
      useNativeDriver: false, // strokeDashoffset isn't driveable natively
    }).start();
  }, [progress, animated]);

  const strokeDashoffset = animated.interpolate({
    inputRange: [0, 100],
    outputRange: [CIRCUMFERENCE, 0],
    extrapolate: "clamp",
  });

  return (
    <View style={styles.container}>
      <Text style={styles.brandText}>Corisco</Text>
      <View style={styles.ring}>
        <Svg width={SIZE} height={SIZE} style={StyleSheet.absoluteFill}>
          <Circle cx={SIZE / 2} cy={SIZE / 2} r={RADIUS} stroke={colors.border} strokeWidth={STROKE_WIDTH} fill="none" />
          <AnimatedCircle
            cx={SIZE / 2}
            cy={SIZE / 2}
            r={RADIUS}
            stroke={colors.accent}
            strokeWidth={STROKE_WIDTH}
            fill="none"
            strokeDasharray={`${CIRCUMFERENCE}, ${CIRCUMFERENCE}`}
            strokeDashoffset={strokeDashoffset}
            strokeLinecap="round"
            transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`}
          />
        </Svg>
        <Text style={styles.percent}>{Math.round(progress)}%</Text>
      </View>
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
    alignItems: "center",
    justifyContent: "center",
  },
  percent: {
    color: colors.textPrimary,
    fontSize: 24,
    fontWeight: "700",
  },
  label: {
    marginTop: spacing.lg,
    color: colors.textSecondary,
    fontSize: 14,
    textAlign: "center",
  },
});
