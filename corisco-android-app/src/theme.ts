// Corisco's dark theme -- a single source of truth for colors so screens stay
// visually consistent. Warm amber accent (the "corisco"/glow) on a near-black
// background.

export const colors = {
  background: "#0B0B0F",
  surface: "#17171D",
  surfaceElevated: "#1F1F27",
  border: "#2A2A33",
  textPrimary: "#F5F5F7",
  textSecondary: "#9B9BA6",
  textMuted: "#6B6B76",
  accent: "#F5B942",
  accentText: "#1A1305",
  error: "#FF6B6B",
  success: "#4ADE80",
} as const;

export const spacing = {
  xs: 4,
  sm: 8,
  md: 16,
  lg: 24,
  xl: 32,
} as const;

export const radii = {
  sm: 8,
  md: 14,
  lg: 20,
  pill: 999,
} as const;
