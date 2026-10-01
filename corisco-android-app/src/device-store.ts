// Persists which hardware signers this phone has already paired with, so
// reconnecting is "pick from a list" instead of re-scanning/re-pairing
// every launch. Only ever stores what the phone itself already knows once
// paired (BLE device id, a user-given label, the wallet's public identity
// key) -- never anything from `crypto-core`'s key material, which this app
// never has in the first place (see ble-hardware-signer.ts's doc comment).

import AsyncStorage from "@react-native-async-storage/async-storage";

const STORAGE_KEY = "corisco.savedDevices";

export type SavedDevice = {
  /** react-native-ble-plx's `Device.id` -- on Android, once bonded, this is
   * the peer's stable identity address (the OS resolves the signer's
   * rotating resolvable-private-address back to it via the IRK exchanged
   * during pairing -- see ble.rs's `resolve_rpa()`), so it stays usable
   * across reconnects without re-scanning. */
  id: string;
  name: string;
  identityPubkey: string;
  lastConnected: number;
};

async function readAll(): Promise<SavedDevice[]> {
  const raw = await AsyncStorage.getItem(STORAGE_KEY);
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

async function writeAll(devices: SavedDevice[]): Promise<void> {
  await AsyncStorage.setItem(STORAGE_KEY, JSON.stringify(devices));
}

export async function listSavedDevices(): Promise<SavedDevice[]> {
  const devices = await readAll();
  return devices.sort((a, b) => b.lastConnected - a.lastConnected);
}

/** A wallet's identity pubkey is what actually identifies it -- the BLE id
 * is just whatever address the radio happens to answer to right now, and
 * can change (a re-flash, an address rotation the OS didn't resolve back
 * to the same bonded identity). Used to stop the same wallet from ending
 * up saved twice under two different ids. */
export async function findDeviceByPubkey(identityPubkey: string): Promise<SavedDevice | undefined> {
  const devices = await readAll();
  return devices.find((d) => d.identityPubkey === identityPubkey);
}

/** Upserts by `id` *and* by `identityPubkey` -- pairing the same physical
 * device again (a rename, a reconnect, or a reconnect under a changed BLE
 * id -- see `findDeviceByPubkey`) updates the existing entry rather than
 * duplicating it, whichever of the two matched. */
export async function saveDevice(device: SavedDevice): Promise<void> {
  const devices = await readAll();
  const next = devices.filter((d) => d.id !== device.id && d.identityPubkey !== device.identityPubkey);
  next.push(device);
  await writeAll(next);
}

export async function touchLastConnected(id: string): Promise<void> {
  const devices = await readAll();
  const found = devices.find((d) => d.id === id);
  if (!found) return;
  found.lastConnected = Date.now();
  await writeAll(devices);
}

export async function removeDevice(id: string): Promise<void> {
  const devices = await readAll();
  await writeAll(devices.filter((d) => d.id !== id));
}
