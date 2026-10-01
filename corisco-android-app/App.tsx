// Corisco -- a self-custodial Lightning wallet where the private keys never
// touch this app: every signing operation routes through the real ESP32
// hardware signer over BLE (`BleHardwareSigner`/`BleSignerConnection`).
// See `docs/architecture.md` for the full request/response flow.

import { useCallback, useEffect, useRef, useState } from "react";
import { StatusBar } from "expo-status-bar";
import { Alert, StyleSheet, Text, TouchableOpacity, View } from "react-native";
import { SparkWallet, SparkWalletEvent, type SparkWallet as SparkWalletType } from "@buildonspark/spark-sdk";
import type { WalletTransfer } from "@buildonspark/spark-sdk/types";
import { BleHardwareSigner } from "./src/ble-hardware-signer";
import { BleSignerConnection, type ScanResult } from "./src/ble-transport";
import { colors, radii, spacing } from "./src/theme";
import {
  findDeviceByPubkey,
  listSavedDevices,
  removeDevice,
  saveDevice,
  touchLastConnected,
  type SavedDevice,
} from "./src/device-store";
import { HomeScreen } from "./src/screens/HomeScreen";
import { NameSignerScreen } from "./src/screens/NameSignerScreen";
import { PairScanScreen } from "./src/screens/PairScanScreen";
import { ReceiveScreen } from "./src/screens/ReceiveScreen";
import { SendScreen } from "./src/screens/SendScreen";
import { SettingsScreen } from "./src/screens/SettingsScreen";
import { SignerListScreen } from "./src/screens/SignerListScreen";
import { SyncingScreen } from "./src/screens/SyncingScreen";
import { TransactionDetailScreen } from "./src/screens/TransactionDetailScreen";
import { fetchBtcPrice } from "./src/price";
import { DEFAULT_SETTINGS, loadSettings, saveSettings, type Settings } from "./src/settings-store";

type Screen = "home" | "receive" | "send" | "settings" | "transaction";

export default function App() {
  const [wallet, setWallet] = useState<SparkWalletType | null>(null);
  const [identityPubkey, setIdentityPubkey] = useState<string | null>(null);
  const [availableSats, setAvailableSats] = useState<bigint | null>(null);
  // `incoming`: sats the server already knows about but that aren't
  // claimed/spendable yet -- distinct from a stuck transfer, this is
  // exactly the gap `claimPending` is working through. `claiming`: true
  // while a claim pass is actually in flight, so a multi-step claim (the
  // balance climbing in increments as each transfer/change output gets
  // claimed one at a time) reads as "still working," not as several
  // separate, confusing partial numbers.
  const [incomingSats, setIncomingSats] = useState<bigint | null>(null);
  const [claiming, setClaiming] = useState(false);
  const [transfers, setTransfers] = useState<WalletTransfer[]>([]);
  const [transfersLoading, setTransfersLoading] = useState(true);
  const [selectedTransfer, setSelectedTransfer] = useState<WalletTransfer | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [initError, setInitError] = useState<string | null>(null);
  const [screen, setScreen] = useState<Screen>("home");

  // Pairing must happen before the wallet can be initialized at all (the
  // signer needs a live connection) -- an explicit user action (tapping
  // the connect button), since scanning/connecting/pairing all need to
  // happen after a real device is in range and powered on.
  const [connecting, setConnecting] = useState(false);
  const [connectingId, setConnectingId] = useState<string | null>(null);
  const [connectStatus, setConnectStatus] = useState("");
  const signerRef = useRef<BleHardwareSigner | null>(null);
  const connRef = useRef<BleSignerConnection | null>(null);

  // Shown full-screen between a successful BLE connection and Home --
  // claiming/optimizing/fetching balance+transfers are real round-trips,
  // not instant, so this gives one honest progress indicator instead of
  // dropping onto Home with stale/zero numbers. `syncProgress` is driven
  // by real stage completions in `connectAndInit` below; 100% lands
  // exactly when the balance and transfer list are actually in hand.
  const [syncing, setSyncing] = useState(false);
  const [syncProgress, setSyncProgress] = useState(0);
  const [syncLabel, setSyncLabel] = useState("");

  // Previously-paired signers (see device-store.ts), so a returning user
  // picks one from a list instead of re-scanning/re-pairing every launch.
  // `pendingNewDevice` gates a one-time "name this signer" screen in
  // between a *first-ever* successful pairing and the normal wallet UI --
  // set once identity pubkey comes back from a connect that wasn't keyed
  // by an already-saved device id.
  const [savedDevices, setSavedDevices] = useState<SavedDevice[]>([]);
  const [loadingSavedDevices, setLoadingSavedDevices] = useState(true);
  const [pendingNewDevice, setPendingNewDevice] = useState<{
    deviceId: string;
    defaultName: string;
    identityPubkey: string;
  } | null>(null);

  useEffect(() => {
    (async () => {
      setSavedDevices(await listSavedDevices());
      setLoadingSavedDevices(false);
    })();
  }, []);

  const forgetDevice = useCallback(async (device: SavedDevice) => {
    await removeDevice(device.id);
    setSavedDevices(await listSavedDevices());
  }, []);

  // Display preferences (settings-store.ts) -- currency, sats/BTC, hide
  // amounts. Purely cosmetic (see that file's doc comment); loaded once on
  // mount, persisted on every change.
  const [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS);
  useEffect(() => {
    (async () => setSettings(await loadSettings()))();
  }, []);
  const updateSettings = useCallback((patch: Partial<Settings>) => {
    setSettings((prev) => {
      const next = { ...prev, ...patch };
      void saveSettings(next);
      return next;
    });
  }, []);

  // BTC/fiat rate for the conversion line under the balance (price.ts) --
  // fetched once a wallet's connected, and again whenever the chosen
  // currency changes, plus a slow background refresh. Display-only: a
  // failed/stale fetch just means the conversion line doesn't show.
  const [btcPrice, setBtcPrice] = useState<number | null>(null);
  useEffect(() => {
    if (!wallet) return;
    let cancelled = false;
    const refresh = async () => {
      const price = await fetchBtcPrice(settings.currency);
      if (!cancelled) setBtcPrice(price);
    };
    void refresh();
    const interval = setInterval(() => void refresh(), 5 * 60_000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [wallet, settings.currency]);

  // First-time (or "pair another") flow: scan for nearby signers and show
  // them as a list to choose from, rather than auto-connecting to the
  // first match (see ble-transport.ts's `scanForCandidates` doc comment
  // for why that list can only show one entry today). `pairConnRef` just
  // holds the scanning connection so it can be stopped on cancel/select --
  // the actual pairing connection is a fresh one made by `connectAndInit`,
  // same as reconnecting to a saved device.
  const [pairScanActive, setPairScanActive] = useState(false);
  const [pairScanning, setPairScanning] = useState(false);
  const [pairCandidates, setPairCandidates] = useState<ScanResult[]>([]);
  const pairConnRef = useRef<BleSignerConnection | null>(null);
  const pairStopRef = useRef<(() => void) | null>(null);

  const startPairScan = useCallback(async () => {
    setInitError(null);
    setPairCandidates([]);
    setPairScanActive(true);
    setPairScanning(true);
    const conn = new BleSignerConnection();
    pairConnRef.current = conn;
    try {
      await conn.ensurePermissions();
      await new Promise<void>((resolve, reject) => {
        pairStopRef.current = conn.scanForCandidates(
          (result) => setPairCandidates((prev) => (prev.some((c) => c.id === result.id) ? prev : [...prev, result])),
          (error) => (error ? reject(error) : resolve()),
        );
      });
    } catch (err) {
      setInitError(String(err));
    } finally {
      setPairScanning(false);
    }
  }, []);

  const cancelPairScan = useCallback(() => {
    pairStopRef.current?.();
    pairStopRef.current = null;
    pairConnRef.current = null;
    setPairScanActive(false);
    setPairScanning(false);
  }, []);

  // Claims any pending incoming transfers, with the refund-tx signatures
  // that requires marked as not needing a physical confirmation tap (see
  // `BleHardwareSigner.withoutSpendConfirmation`'s doc comment) -- this is
  // our replacement for the SDK's own uncontrollable background auto-claim,
  // disabled in `connectAndInit` below. `claimTransfers` is a private SDK
  // method (not in its public type surface) called via a cast; errors are
  // surfaced via Alert rather than swallowed, since a stuck pending
  // transfer with no visible cause is exactly what motivated writing this.
  //
  // `claimTransfers()` itself already loops through every batch it can see
  // as of one internal snapshot, but a transfer that only becomes
  // claimable *after* that snapshot (change from a payment still settling,
  // a staggered second leg on regtest) has to wait for a whole separate
  // call. Looping here too -- immediately retrying while a pass is still
  // finding something -- means a burst of several transfers converges as
  // fast as the network allows instead of one wave per periodic tick (see
  // the 10s interval below, which stays as the steady-state safety net for
  // whatever shows up well after this loop has already settled).
  const claimPending = useCallback(async (w: SparkWalletType, signer: BleHardwareSigner) => {
    setClaiming(true);
    try {
      while (true) {
        const claimed = await signer.withoutSpendConfirmation(() =>
          (w as unknown as { claimTransfers(): Promise<string[]> }).claimTransfers(),
        );
        if (claimed.length === 0) break;
      }
    } catch (claimErr) {
      console.warn("claimTransfers failed:", claimErr);
      Alert.alert("Claim failed", String(claimErr));
    } finally {
      setClaiming(false);
    }
  }, []);

  // Consolidates the wallet's leaves into fewer, larger ones. Matters
  // because every leaf a payment needs to spend costs its own full BLE +
  // network signing round-trip (each gated on a physical device
  // confirmation, see `withSpendContext`'s doc comment) -- a wallet
  // fragmented into many small leaves makes an otherwise-simple payment
  // slow and repeatedly interrupt the user for what's really one spend.
  //
  // The SDK has its own built-in auto-optimize, but it fires from a
  // callback wired up internally at `SparkWallet` construction time (see
  // `LeafManager`'s constructor call in the SDK source) -- there's no way
  // to route *that* trigger through `withoutSpendConfirmation`, so if it
  // ever decided consolidation was needed it would produce an unexplained
  // confirm-screen prompt with nothing on the app screen to say why.
  // Calling the same public `optimizeLeaves()` method ourselves, on our
  // own schedule, keeps this fully under our control -- same reasoning as
  // disabling the SDK's own background auto-claim in `connectAndInit`.
  const optimizePending = useCallback(async (w: SparkWalletType, signer: BleHardwareSigner) => {
    try {
      await signer.withoutSpendConfirmation(async () => {
        for await (const _step of w.optimizeLeaves()) {
          // Progress only (`{step, total, controller}`) -- nothing to
          // show for background maintenance the user didn't initiate.
        }
      });
    } catch (optimizeErr) {
      // Not surfaced via Alert like a failed claim -- this is best-effort
      // housekeeping, not something blocking the user's own action, and
      // leaves that don't get consolidated this pass just cost a bit more
      // time on the next payment rather than being stuck.
      console.warn("optimizeLeaves failed:", optimizeErr);
    }
  }, []);

  const refreshBalance = useCallback(async (w: SparkWalletType) => {
    const balance = await w.getBalance();
    setAvailableSats(balance.satsBalance.available);
    setIncomingSats(balance.satsBalance.incoming);
  }, []);

  const refreshTransfers = useCallback(async (w: SparkWalletType) => {
    setTransfersLoading(true);
    try {
      const { transfers: recent } = await w.getTransfers(settings.showLastXTransactions, 0);
      setTransfers(recent);
    } finally {
      setTransfersLoading(false);
    }
  }, []);

  const connectAndInit = useCallback(async (targetDeviceId: string) => {
    setConnecting(true);
    setConnectingId(targetDeviceId);
    setInitError(null);
    try {
      const conn = new BleSignerConnection();
      const { deviceId, deviceName } = await conn.connect(setConnectStatus, targetDeviceId);

      setConnectStatus("Starting wallet...");
      const signer = new BleHardwareSigner(conn);
      const { wallet: w } = await SparkWallet.initialize({
        signer,
        options: { network: "REGTEST", signerWithPreExistingKeys: true },
      });
      setWallet(w);
      signerRef.current = signer;
      connRef.current = conn;
      // Clears whatever screen led here (PairScanScreen in particular --
      // see `selectCandidate`, which deliberately leaves this `true`
      // through the whole connect attempt so its own connecting/spinner
      // UI stays visible) now that a wallet exists and every subsequent
      // render will skip past the `!wallet` branch entirely regardless.
      setPairScanActive(false);
      setSyncing(true);
      setSyncProgress(10);
      setSyncLabel("Preparing wallet...");

      // The SDK's own background auto-claim (`startPeriodicClaimTransfers`,
      // unconditionally started for React Native, private -- not
      // otherwise controllable) calls `claimTransfers()` on its own timer
      // with no way to mark those calls as not needing spend confirmation
      // -- every refund-tx signature a claim needs would gate on a
      // physical tap. Disabled here in favor of our own periodic claim
      // below, which we DO control the call site of.
      const w2 = w as unknown as { claimTransfersInterval: ReturnType<typeof setInterval> | null };
      if (w2.claimTransfersInterval) {
        clearInterval(w2.claimTransfersInterval);
        w2.claimTransfersInterval = null;
      }

      const pubkey = await w.getIdentityPublicKey();
      setIdentityPubkey(pubkey);
      setSyncProgress(25);

      if (savedDevices.some((d) => d.id === deviceId)) {
        await touchLastConnected(deviceId);
      } else {
        // Not a known BLE id -- but the wallet it unlocks might still be
        // one already saved under a *different* id (its BLE address
        // rotated, or the phone bonded to it again after a re-flash).
        // What actually identifies a wallet is its identity pubkey, not
        // the transient radio address, so check that before treating this
        // as a brand-new pairing -- otherwise the same wallet ends up
        // saved twice (see device-store.ts's `findDeviceByPubkey`).
        const existing = await findDeviceByPubkey(pubkey);
        if (existing) {
          await saveDevice({ ...existing, id: deviceId, lastConnected: Date.now() });
          setSavedDevices(await listSavedDevices());
        } else {
          // Genuinely never paired before -- name it before dropping into
          // the normal wallet UI (see NameSignerScreen).
          setPendingNewDevice({ deviceId, defaultName: deviceName ?? "Spark Signer", identityPubkey: pubkey });
        }
      }

      setSyncLabel("Checking for pending payments...");
      await claimPending(w, signer);
      setSyncProgress(60);

      setSyncLabel("Optimizing wallet...");
      await optimizePending(w, signer);
      setSyncProgress(80);

      setSyncLabel("Loading balance and activity...");
      await Promise.all([refreshBalance(w), refreshTransfers(w)]);
      // Only now -- once the numbers Home is about to show are actually in
      // hand -- does the ring complete. A short pause lets the user
      // register "100%" before it's swapped out for the real screen.
      setSyncProgress(100);
      setSyncLabel("Ready");
      await new Promise((resolve) => setTimeout(resolve, 400));
    } catch (err) {
      setInitError(String(err));
    } finally {
      setConnecting(false);
      setConnectingId(null);
      setLoading(false);
      setSyncing(false);
    }
  }, [savedDevices, refreshBalance, refreshTransfers, claimPending, optimizePending]);

  // Tears down the live BLE connection and drops back to the signer picker
  // -- e.g. to switch which physical device this session is talking to,
  // without force-quitting the app. Resets every piece of per-wallet state
  // this component holds so the next `connectAndInit` starts from a clean
  // slate, same as a fresh app launch.
  const disconnectWallet = useCallback(async () => {
    try {
      await connRef.current?.disconnect();
    } catch (err) {
      console.warn("disconnect failed:", err);
    }
    connRef.current = null;
    signerRef.current = null;
    setWallet(null);
    setIdentityPubkey(null);
    setAvailableSats(null);
    setIncomingSats(null);
    setTransfers([]);
    setTransfersLoading(true);
    setScreen("home");
    setLoading(true);
    setSyncing(false);
    setSyncProgress(0);
    setPairScanActive(false);
  }, []);

  const selectCandidate = useCallback((candidate: ScanResult) => {
    // Deliberately does NOT hide the scan screen here -- `PairScanScreen`
    // already renders its own per-row spinner + status text while
    // `connecting` is true (passed through below), and hiding it
    // immediately meant that progress UI never got a chance to show at
    // all: with zero saved devices, the very next render fell straight
    // through to the bare "Connect to Signer" screen (no progress
    // indicator there), which read as "got bounced back to the start."
    // `connectAndInit` itself clears this once a wallet actually exists.
    pairStopRef.current?.();
    pairStopRef.current = null;
    pairConnRef.current = null;
    void connectAndInit(candidate.id);
  }, [connectAndInit]);

  const finishNaming = useCallback(async (name: string) => {
    if (!pendingNewDevice) return;
    await saveDevice({
      id: pendingNewDevice.deviceId,
      name,
      identityPubkey: pendingNewDevice.identityPubkey,
      lastConnected: Date.now(),
    });
    setPendingNewDevice(null);
    setSavedDevices(await listSavedDevices());
  }, [pendingNewDevice]);

  // Also claims pending transfers -- this is the app's one refresh action
  // (pull-to-refresh and the explicit button in HomeScreen both call it).
  // Deliberately not on a background timer: BLE round-trips aren't free,
  // and polling regardless of whether the user is even looking at the
  // screen is pure overhead for no benefit over just checking when asked.
  const onRefresh = useCallback(async () => {
    if (!wallet || !signerRef.current) return;
    setRefreshing(true);
    try {
      await claimPending(wallet, signerRef.current);
      await Promise.all([refreshBalance(wallet), refreshTransfers(wallet)]);
    } finally {
      setRefreshing(false);
    }
  }, [wallet, claimPending, refreshBalance, refreshTransfers]);

  // Live updates: the SDK keeps a gRPC event stream open and emits these as
  // things happen server-side (an incoming payment gets claimed, a deposit
  // confirms, etc.) -- no polling needed. BalanceUpdate carries the new
  // balance directly; TransferClaimed just tells us to go re-fetch the
  // transfer list (its own payload is only an id + updated total).
  useEffect(() => {
    if (!wallet) return;
    const onBalanceUpdate = (balance: { available: bigint; incoming: bigint }) => {
      setAvailableSats(balance.available);
      setIncomingSats(balance.incoming);
    };
    const onTransferClaimed = () => {
      void refreshTransfers(wallet);
    };
    wallet.on(SparkWalletEvent.BalanceUpdate, onBalanceUpdate);
    wallet.on(SparkWalletEvent.TransferClaimed, onTransferClaimed);
    return () => {
      wallet.off(SparkWalletEvent.BalanceUpdate, onBalanceUpdate);
      wallet.off(SparkWalletEvent.TransferClaimed, onTransferClaimed);
    };
  }, [wallet, refreshTransfers]);

  // Slower cadence than claiming -- consolidation is real signing work
  // (every leaf-swap batch is its own set of BLE round-trips), not just a
  // cheap status check, so running it as often as the claim poll would
  // add to the exact overhead this exists to reduce.
  useEffect(() => {
    if (!wallet || !signerRef.current) return;
    const signer = signerRef.current;
    const interval = setInterval(() => {
      void optimizePending(wallet, signer);
    }, 60_000);
    return () => clearInterval(interval);
  }, [wallet, optimizePending]);

  if (!wallet) {
    if (pairScanActive) {
      return (
        <>
          <PairScanScreen
            scanning={pairScanning}
            candidates={pairCandidates}
            connecting={connecting}
            connectingId={connectingId}
            connectStatus={connectStatus}
            error={initError}
            onSelect={selectCandidate}
            onRescan={() => void startPairScan()}
            onCancel={cancelPairScan}
          />
          <StatusBar style="light" />
        </>
      );
    }
    if (!loadingSavedDevices && savedDevices.length > 0) {
      return (
        <>
          <SignerListScreen
            devices={savedDevices}
            connecting={connecting}
            connectingId={connectingId}
            connectStatus={connectStatus}
            initError={initError}
            onSelect={(device) => void connectAndInit(device.id)}
            onForget={(device) => void forgetDevice(device)}
            onPairNew={() => void startPairScan()}
          />
          <StatusBar style="light" />
        </>
      );
    }
    return (
      <View style={styles.centered}>
        <Text style={styles.brandText}>Corisco</Text>
        {initError ? (
          <Text style={styles.errorBody}>{initError}</Text>
        ) : (
          <Text style={styles.instructionText}>Connect to your Spark Signer device to get started.</Text>
        )}
        <TouchableOpacity style={styles.connectButton} onPress={() => void startPairScan()}>
          <Text style={styles.connectButtonText}>{initError ? "Try again" : "Connect to Signer"}</Text>
        </TouchableOpacity>
        <StatusBar style="light" />
      </View>
    );
  }

  if (pendingNewDevice) {
    return (
      <NameSignerScreen
        defaultName={pendingNewDevice.defaultName}
        identityPubkey={pendingNewDevice.identityPubkey}
        onSave={(name) => void finishNaming(name)}
      />
    );
  }

  if (syncing) {
    return <SyncingScreen progress={syncProgress} label={syncLabel} />;
  }

  return (
    <View style={styles.root}>
      {screen === "home" && (
        <HomeScreen
          identityPubkey={identityPubkey}
          availableSats={availableSats}
          incomingSats={incomingSats}
          claiming={claiming}
          loading={loading}
          refreshing={refreshing}
          onRefresh={onRefresh}
          onReceive={() => setScreen("receive")}
          onSend={() => setScreen("send")}
          onSettings={() => setScreen("settings")}
          onSelectTransfer={(transfer) => {
            setSelectedTransfer(transfer);
            setScreen("transaction");
          }}
          transfers={transfers}
          transfersLoading={transfersLoading}
          settings={settings}
          btcPrice={btcPrice}
        />
      )}
      {screen === "receive" && wallet && (
        <ReceiveScreen wallet={wallet} onBack={() => setScreen("home")} />
      )}
      {screen === "send" && wallet && (
        <SendScreen
          wallet={wallet}
          // Non-null: set in the same synchronous step as `wallet` in
          // connectAndInit, and this screen only renders once `wallet` is
          // truthy.
          signer={signerRef.current!}
          onBack={() => setScreen("home")}
          onPaid={() => {
            void onRefresh();
          }}
        />
      )}
      {screen === "settings" && (
        <SettingsScreen
          settings={settings}
          onChange={updateSettings}
          onDisconnect={() => void disconnectWallet()}
          onBack={() => setScreen("home")}
        />
      )}
      {screen === "transaction" && selectedTransfer && (
        <TransactionDetailScreen
          transfer={selectedTransfer}
          settings={settings}
          btcPrice={btcPrice}
          onBack={() => setScreen("home")}
        />
      )}
      <StatusBar style="light" />
    </View>
  );
}

const styles = StyleSheet.create({
  root: {
    flex: 1,
    backgroundColor: colors.background,
  },
  centered: {
    flex: 1,
    backgroundColor: colors.background,
    alignItems: "center",
    justifyContent: "center",
    padding: spacing.lg,
  },
  errorTitle: {
    color: colors.textPrimary,
    fontSize: 20,
    fontWeight: "700",
    marginBottom: spacing.md,
  },
  errorBody: {
    color: colors.error,
    fontSize: 13,
    textAlign: "center",
  },
  brandText: {
    color: colors.accent,
    fontSize: 22,
    fontWeight: "700",
    letterSpacing: 1,
    marginBottom: spacing.lg,
  },
  instructionText: {
    color: colors.textSecondary,
    fontSize: 14,
    textAlign: "center",
    marginBottom: spacing.lg,
  },
  connectButton: {
    backgroundColor: colors.accent,
    borderRadius: radii.pill,
    paddingVertical: spacing.md,
    paddingHorizontal: spacing.xl,
  },
  connectButtonText: {
    color: colors.accentText,
    fontSize: 15,
    fontWeight: "700",
  },
});
