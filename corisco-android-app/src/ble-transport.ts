// Talks to the hardware signer's real BLE GATT signing service (see
// `firmware-core/src/ble.rs`). Mirrors that file's own "Framing" doc
// comment exactly: every message (request or response) is
// `[len_lo, len_hi, ...first-chunk-payload]` then raw continuation
// chunks, sized to the negotiated ATT MTU minus 3 bytes of overhead.
//
// UUIDs below are the same 128-bit values as `ble.rs`'s `SERVICE_UUID`/
// `REQUEST_CHARACTERISTIC_UUID`/`RESPONSE_CHARACTERISTIC_UUID` constants,
// just written as standard dash-separated UUID strings (`ble.rs`'s `uuid()`
// helper does `id.to_le_bytes()` on the same `u128` literals -- which is
// exactly how a standard UUID string's bytes end up on the wire, so these
// strings and those Rust hex literals denote the identical UUID).

import { PermissionsAndroid, Platform } from "react-native";
import { BleManager, type Device, type Subscription } from "react-native-ble-plx";
import { decodeResponse, encodeRequest, type Request, type Response } from "./postcard";

/** Android 12+ (API 31+) treats BLUETOOTH_SCAN/BLUETOOTH_CONNECT as
 * runtime-dangerous permissions -- declaring them in the manifest (which
 * the `react-native-ble-plx` config plugin already does) isn't enough,
 * they must also be requested at runtime or the OS reports the app as
 * "Unauthorized" for BLE regardless of manifest content. Below API 31,
 * `neverForLocation: true` (see app.json) means no location permission is
 * needed either, so this is a no-op there (PermissionsAndroid resolves
 * unknown/inapplicable permissions as already granted).
 */
async function ensureAndroidBlePermissions(): Promise<void> {
  if (Platform.OS !== "android") return;
  const granted = await PermissionsAndroid.requestMultiple([
    PermissionsAndroid.PERMISSIONS.BLUETOOTH_SCAN,
    PermissionsAndroid.PERMISSIONS.BLUETOOTH_CONNECT,
  ]);
  const denied = Object.entries(granted).filter(([, result]) => result !== PermissionsAndroid.RESULTS.GRANTED);
  if (denied.length > 0) {
    throw new Error(
      `Bluetooth permission denied: ${denied.map(([perm]) => perm).join(", ")}. Enable it in Settings and try again.`,
    );
  }
}

export type ScanResult = { id: string; name: string };

export const SERVICE_UUID = "5f4b2a9e-7c3d-4e8f-a1b6-d09c2e7f4a5b";
const REQUEST_CHARACTERISTIC_UUID = "8e2c6f0a-4b7d-4c9e-9a3f-5d1e8b6c2a70";
const RESPONSE_CHARACTERISTIC_UUID = "3a9d7e1c-5b4f-4a8d-8c2e-6f0b9d3a7c50";
const DEVICE_NAME = "SparkHW";
const REQUESTED_MTU = 247; // "commonly negotiate up to ~185-247" -- ble.rs's own doc comment
const ATT_OVERHEAD = 3;
const DEFAULT_ATT_MTU = 23;

function bytesToBase64(bytes: Uint8Array): string {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const b0 = bytes[i]!;
    const b1 = i + 1 < bytes.length ? bytes[i + 1]! : undefined;
    const b2 = i + 2 < bytes.length ? bytes[i + 2]! : undefined;
    out += chars[b0 >> 2];
    out += chars[((b0 & 0x03) << 4) | (b1 === undefined ? 0 : b1 >> 4)];
    out += b1 === undefined ? "=" : chars[((b1 & 0x0f) << 2) | (b2 === undefined ? 0 : b2 >> 6)];
    out += b2 === undefined ? "=" : chars[b2 & 0x3f];
  }
  return out;
}

function base64ToBytes(b64: string): Uint8Array {
  const chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const clean = b64.replace(/=+$/, "");
  const out: number[] = [];
  let buffer = 0;
  let bits = 0;
  for (const c of clean) {
    const val = chars.indexOf(c);
    if (val === -1) continue;
    buffer = (buffer << 6) | val;
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out.push((buffer >> bits) & 0xff);
    }
  }
  return new Uint8Array(out);
}

/** Mirrors `ble.rs`'s `RequestReassembly` -- buffers indication frames
 * until a full length-prefixed message has arrived. */
class ResponseReassembly {
  private expectedLen: number | null = null;
  private buf: number[] = [];

  /** Returns the complete message once enough frames have arrived, else `null`. */
  push(frame: Uint8Array): Uint8Array | null {
    if (this.expectedLen === null) {
      if (frame.length < 2) return null;
      this.expectedLen = frame[0]! | (frame[1]! << 8);
      this.buf = Array.from(frame.slice(2));
    } else {
      this.buf.push(...frame);
    }
    if (this.buf.length < this.expectedLen) return null;
    const msg = new Uint8Array(this.buf.slice(0, this.expectedLen));
    this.expectedLen = null;
    this.buf = [];
    return msg;
  }
}

export class BleSignerConnection {
  private manager = new BleManager();
  private device: Device | null = null;
  private mtu = DEFAULT_ATT_MTU;
  private responseSub: Subscription | null = null;
  private reassembly = new ResponseReassembly();
  private pendingResolve: ((r: Response) => void) | null = null;
  private pendingReject: ((e: unknown) => void) | null = null;
  // The device can only service one request at a time (single reassembly
  // buffer, single response characteristic -- see ble.rs's "Only one
  // request may be in flight" doc comment), but callers here don't know
  // that: the SDK legitimately fires multiple signer calls concurrently
  // (e.g. during SparkWallet.initialize's setup). Queuing here keeps the
  // wire serialized without leaking that constraint as a thrown error.
  private queue: Promise<unknown> = Promise.resolve();

  /** Requests Android's runtime BLE permissions (a no-op elsewhere, and a
   * no-op if already granted) -- call before scanning or connecting. */
  async ensurePermissions(): Promise<void> {
    await ensureAndroidBlePermissions();
  }

  /** Scans for a device named `DEVICE_NAME`, resolving once found (not yet
   * connected). Used only as `connect()`'s fallback when a saved device id
   * doesn't respond directly -- for first-time pairing, `scanForCandidates`
   * below is what drives the picker screen instead of auto-selecting the
   * first match. */
  private scanForDevice(onStatus?: (msg: string) => void): Promise<Device> {
    onStatus?.("Scanning...");
    return new Promise<Device>((resolve, reject) => {
      const timeout = setTimeout(() => {
        this.manager.stopDeviceScan();
        reject(new Error(`No device named "${DEVICE_NAME}" found -- is it powered on and advertising?`));
      }, 15000);
      this.manager.startDeviceScan([SERVICE_UUID], null, (error, scanned) => {
        if (error) {
          clearTimeout(timeout);
          reject(error);
          return;
        }
        if (scanned && scanned.name === DEVICE_NAME) {
          clearTimeout(timeout);
          this.manager.stopDeviceScan();
          resolve(scanned);
        }
      });
    });
  }

  /** Scans for nearby signers and reports each new one found, for a picker
   * screen to show during first-time pairing (rather than silently
   * auto-connecting to the first match, which is what `scanForDevice`
   * above does for the saved-device fallback path). Matches on advertised
   * name only for now -- every real signer currently advertises the same
   * fixed `DEVICE_NAME`, so today this can only ever surface one distinct
   * candidate; once the firmware gives each device its own default name,
   * this same scan naturally starts distinguishing multiple nearby
   * signers with no protocol change needed here.
   *
   * Calls `onDone` (with an error, if the scan itself failed) once scanning
   * stops, whether from the timeout or an explicit `stop()` call. Returns
   * that `stop` function. */
  scanForCandidates(onFound: (result: ScanResult) => void, onDone: (error?: unknown) => void, timeoutMs = 15000): () => void {
    const seen = new Set<string>();
    let stopped = false;
    const stop = () => {
      if (stopped) return;
      stopped = true;
      clearTimeout(timeout);
      this.manager.stopDeviceScan();
    };
    const timeout = setTimeout(() => {
      stop();
      onDone();
    }, timeoutMs);
    this.manager.startDeviceScan([SERVICE_UUID], null, (error, scanned) => {
      if (error) {
        stop();
        onDone(error);
        return;
      }
      if (scanned && scanned.name === DEVICE_NAME && !seen.has(scanned.id)) {
        seen.add(scanned.id);
        onFound({ id: scanned.id, name: scanned.name });
      }
    });
    return stop;
  }

  /** Connects to a specific, already-known device id -- either a
   * previously-bonded saved signer (device-store.ts) or one just found via
   * `scanForCandidates` -- and subscribes to the response characteristic.
   * `onStatus` is called with short human-readable progress updates. */
  private async connectToId(deviceId: string, onStatus?: (msg: string) => void): Promise<{ deviceId: string; deviceName: string | null }> {
    onStatus?.("Connecting...");
    const connected = await this.manager.connectToDevice(deviceId, { requestMTU: REQUESTED_MTU });
    this.device = connected;
    // iOS negotiates MTU automatically and this call is a no-op there;
    // on Android, `requestMTU` above already asked for it at connect time,
    // this just reads back what was actually granted.
    const withMtu = await this.manager.requestMTUForDevice(connected.id, REQUESTED_MTU).catch(() => connected);
    this.mtu = withMtu.mtu ?? DEFAULT_ATT_MTU;

    onStatus?.("Discovering services...");
    await this.manager.discoverAllServicesAndCharacteristicsForDevice(connected.id);

    this.responseSub = this.manager.monitorCharacteristicForDevice(
      connected.id,
      SERVICE_UUID,
      RESPONSE_CHARACTERISTIC_UUID,
      (error, characteristic) => {
        if (error) {
          this.pendingReject?.(error);
          return;
        }
        if (!characteristic?.value) return;
        const frame = base64ToBytes(characteristic.value);
        const complete = this.reassembly.push(frame);
        if (complete) {
          try {
            const response = decodeResponse(complete);
            this.pendingResolve?.(response);
          } catch (e) {
            this.pendingReject?.(e);
          } finally {
            this.pendingResolve = null;
            this.pendingReject = null;
          }
        }
      },
    );

    onStatus?.("Connected");
    return { deviceId: connected.id, deviceName: connected.name };
  }

  /** Connects to `deviceId` -- pairing the OS's native passkey dialog the
   * first time (the code shown there must match what's on the device's
   * own screen); subsequent connects to an already-bonded device reconnect
   * silently. Works whether `deviceId` is a previously-bonded saved
   * signer's stable identity address (see device-store.ts's `SavedDevice.id`
   * doc comment) or a freshly-scanned candidate's id -- if the direct
   * connect fails (device went out of range, address book entry gone
   * stale) this falls back to a normal scan-by-name rather than failing
   * outright. Returns the id/name of whatever device actually ended up
   * connected, in case that differs from `deviceId` because of the
   * fallback. */
  async connect(onStatus: ((msg: string) => void) | undefined, deviceId: string): Promise<{ deviceId: string; deviceName: string | null }> {
    await this.ensurePermissions();
    try {
      return await this.connectToId(deviceId, onStatus);
    } catch {
      onStatus?.("Signer not responding, scanning...");
      const scanned = await this.scanForDevice(onStatus);
      return this.connectToId(scanned.id, onStatus);
    }
  }

  /** Sends one request, chunked per the negotiated MTU, and waits for the
   * (also chunked) response. Concurrent callers are queued, not rejected --
   * see the `queue` field's comment -- so this is safe to call without the
   * caller worrying about another request already being in flight. */
  async request(req: Request): Promise<Response> {
    const run = this.queue.then(() => this.sendRequest(req));
    // Swallow the rejection in the queue chain itself (a failed request
    // must not wedge every request queued after it) -- the real error
    // still propagates to this call's own caller via `run` below.
    this.queue = run.catch(() => undefined);
    return run;
  }

  private async sendRequest(req: Request): Promise<Response> {
    if (!this.device) throw new Error("BleSignerConnection: not connected");

    const payload = encodeRequest(req);
    const chunkSize = Math.max(1, this.mtu - ATT_OVERHEAD);

    const len = new Uint8Array([payload.length & 0xff, (payload.length >> 8) & 0xff]);
    const firstPayloadRoom = Math.max(0, chunkSize - 2);
    const frames: Uint8Array[] = [];
    const first = new Uint8Array(2 + Math.min(payload.length, firstPayloadRoom));
    first.set(len, 0);
    first.set(payload.slice(0, firstPayloadRoom), 2);
    frames.push(first);
    for (let offset = firstPayloadRoom; offset < payload.length; offset += chunkSize) {
      frames.push(payload.slice(offset, offset + chunkSize));
    }

    const responsePromise = new Promise<Response>((resolve, reject) => {
      this.pendingResolve = resolve;
      this.pendingReject = reject;
    });

    for (const frame of frames) {
      await this.manager.writeCharacteristicWithResponseForDevice(
        this.device.id,
        SERVICE_UUID,
        REQUEST_CHARACTERISTIC_UUID,
        bytesToBase64(frame),
      );
    }

    const response = await responsePromise;
    if (response.type === "Error") {
      throw new Error(`device error: ${response.message}`);
    }
    return response;
  }

  async disconnect(): Promise<void> {
    this.responseSub?.remove();
    this.responseSub = null;
    if (this.device) {
      await this.manager.cancelDeviceConnection(this.device.id).catch(() => {});
      this.device = null;
    }
  }
}
