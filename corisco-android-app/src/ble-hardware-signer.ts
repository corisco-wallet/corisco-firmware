// Real hardware transport for `SparkSigner`, over `ble-transport.ts`'s
// connection to the firmware's actual BLE GATT service. Built on
// `signerWithPreExistingKeys` -- no private key material ever touches
// this process; every signing operation is a round trip to the device.
//
// Covers identity/deposit/leaf public keys, FROST commit/sign (including
// the adaptor-signature path Lightning leaf-swaps need), identity-key
// Schnorr/ECDSA signing, and the full leaf-ownership-transfer "key tweak"
// pair: `subtractAndSplitSecretWithProofsGivenDerivations` + `decryptEcies`
// (receiving/claiming) and `subtractSplitAndEncrypt` (sending a leaf-swap
// payment, i.e. one whose leaves don't sum to the exact invoice amount).
//
// Still NOT covered: `subtractPrivateKeysGivenDerivationPaths` and
// `splitSecretWithProofs` standalone -- neither is on any path exercised
// yet; they fall through to `DefaultSparkSigner`'s base implementation,
// which throws (no local private key material to read, by design) rather
// than silently failing insecurely. Add them here the same way once
// something real needs them.

import {
  DefaultSparkSigner,
  KeyDerivationType,
  type KeyDerivation,
  type SignFrostParams,
  type SigningCommitment,
  type SigningCommitmentWithOptionalNonce,
  type SplitSecretWithProofsParams,
  type SubtractSplitAndEncryptParams,
  type SubtractSplitAndEncryptResult,
  type VerifiableSecretShare,
} from "@buildonspark/spark-sdk";
import { BleSignerConnection } from "./ble-transport";
import type { KeyDerivationRefWire, ShareWire, StatechainCommitmentWire } from "./postcard";

function hexToBytes(hex: string): Uint8Array {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(hex.substring(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

/** Translates the SDK's `KeyDerivation` union into `postcard.ts`'s binary
 * wire shape (raw ciphertext bytes, matching `ble.rs`'s `KeyDerivationRef`). */
function toKeyDerivationRefWire(kd: KeyDerivation): KeyDerivationRefWire {
  switch (kd.type) {
    case KeyDerivationType.LEAF:
      return { type: "Leaf", leafId: kd.path };
    case KeyDerivationType.DEPOSIT:
      return { type: "Deposit" };
    case KeyDerivationType.STATIC_DEPOSIT:
      return { type: "StaticDeposit", idx: kd.path };
    case KeyDerivationType.ECIES:
      return { type: "Ecies", ciphertext: kd.path };
    case KeyDerivationType.RANDOM:
      return { type: "Random" };
  }
}

function sharesFromWire(shares: ShareWire[]): VerifiableSecretShare[] {
  return shares.map((s) => ({ threshold: s.threshold, index: s.index, share: s.share, proofs: s.proofs }));
}

export class BleHardwareSigner extends DefaultSparkSigner {
  // Keyed by object identity, same as HardwareBridgeSigner -- the SDK
  // hands back the exact same SigningCommitment object in signFrost's
  // selfCommitment later.
  private pendingCommitmentIds = new Map<SigningCommitment, number>();

  // Depth counter, not a plain bool, so nested/concurrent claim-related
  // calls (e.g. one claim's Promise.all of several leaves) don't have one
  // finishing early re-enable confirmation while a sibling call is still
  // in flight. See ble.rs's `requires_confirmation` doc comment for why
  // this exists at all: gating every `Sign` uniformly (including the
  // refund-tx signatures a claim needs, which don't move value anywhere)
  // means claiming needs enough sequential physical taps to risk a BLE
  // disconnect mid-claim.
  private spendConfirmationSuppressedDepth = 0;

  // Set only while a `withSpendContext`-wrapped send is in flight; `null`
  // otherwise. Just the display context -- deliberately does NOT also
  // carry any "skip confirmation for this leaf" logic: there is no way to
  // verify from a `SignFrostParams` alone that a given `Sign` call
  // corresponds to the leaf the app claims it does, so a filter like that
  // is unsafe -- it would let a real spend go through with no on-device
  // confirmation at all.
  private spendContext: { amountSats: bigint; destination: string } | null = null;

  constructor(private conn: BleSignerConnection) {
    super();
  }

  /** Runs `fn` with `Sign` requests it triggers marked as not needing an
   * on-device confirmation tap -- for the SDK's own internal protocol
   * signing (refund transactions during a claim), never for anything that
   * actually spends the wallet's funds. Callers are responsible for only
   * wrapping claim-shaped operations; this class has no way to verify
   * that from a `SignFrostParams` alone (see ble.rs's doc comment). */
  async withoutSpendConfirmation<T>(fn: () => Promise<T>): Promise<T> {
    this.spendConfirmationSuppressedDepth++;
    try {
      return await fn();
    } finally {
      this.spendConfirmationSuppressedDepth--;
    }
  }

  /** Runs `fn` (a real outgoing payment, e.g. `payLightningInvoice`) with
   * `amountSats`/`destination` attached to every `Sign` request it
   * triggers, all of them requiring on-device confirmation.
   *
   * Deliberately does NOT try to guess which underlying `signFrost` call
   * is "the real spend" and skip confirmation on the rest, the way
   * `withoutSpendConfirmation` does for claiming: `payLightningInvoice`'s
   * internal signing order isn't independently verifiable from here (its
   * leaf-swap for a non-exact amount likely signs a freshly-swapped leaf
   * for the real payment, which any "already-known-leaf" filter would
   * misclassify as safe to skip). A hardware wallet that silently signs a
   * real spend is a much worse failure than one that asks more than
   * once, so this errs unconditionally toward asking every time.
   * `knownLeafIds` is unused for now -- kept in the parameter shape for a
   * future dedup that only activates once the signing order above is
   * actually confirmed, not assumed. */
  async withSpendContext<T>(
    { amountSats, destination }: { amountSats: bigint; destination: string; knownLeafIds?: ReadonlySet<string> },
    fn: () => Promise<T>,
  ): Promise<T> {
    const previous = this.spendContext;
    this.spendContext = { amountSats, destination };
    try {
      return await fn();
    } finally {
      this.spendContext = previous;
    }
  }

  override async getRandomSigningCommitment(): Promise<SigningCommitmentWithOptionalNonce> {
    const resp = await this.conn.request({ type: "Commit" });
    if (resp.type !== "Commit") throw new Error(`unexpected response to Commit: ${resp.type}`);
    const commitment: SigningCommitment = { hiding: resp.hiding, binding: resp.binding };
    this.pendingCommitmentIds.set(commitment, resp.commitmentId);
    return { commitment };
  }

  override async signFrost(params: SignFrostParams): Promise<Uint8Array> {
    if (params.keyDerivation.type !== KeyDerivationType.LEAF) {
      throw new Error(
        `BleHardwareSigner only supports LEAF signing (the only scheme a single hardware-wallet device needs), got: ${params.keyDerivation.type}`,
      );
    }
    const leafId = params.keyDerivation.path;

    const commitmentId = this.pendingCommitmentIds.get(params.selfCommitment.commitment);
    if (commitmentId === undefined) {
      throw new Error(
        "signFrost called with a selfCommitment that didn't come from this signer's getRandomSigningCommitment()",
      );
    }
    this.pendingCommitmentIds.delete(params.selfCommitment.commitment);

    const statechainCommitments: StatechainCommitmentWire[] = Object.entries(
      params.statechainCommitments ?? {},
    ).map(([idHex, commitment]) => ({
      identifier: hexToBytes(idHex),
      hiding: commitment.hiding,
      binding: commitment.binding,
    }));

    // Every Sign during an active spend context requires confirmation --
    // see `withSpendContext`'s doc comment for why this doesn't try to
    // filter which one is "the real spend" the way claiming's
    // `withoutSpendConfirmation` safely can.
    const requiresConfirmation = this.spendConfirmationSuppressedDepth === 0;

    const resp = await this.conn.request({
      type: "Sign",
      commitmentId,
      leafId,
      message: params.message,
      statechainCommitments,
      verifyingKey: params.verifyingKey,
      // Same empty-Uint8Array-is-truthy caveat as HardwareBridgeSigner's
      // signFrost -- see that file's comment on this exact gotcha.
      adaptorPublicKey: params.adaptorPubKey && params.adaptorPubKey.length > 0 ? params.adaptorPubKey : undefined,
      requiresConfirmation,
      amountSats: requiresConfirmation ? this.spendContext?.amountSats : undefined,
      destination: requiresConfirmation ? this.spendContext?.destination : undefined,
    });
    if (resp.type !== "Sign") throw new Error(`unexpected response to Sign: ${resp.type}`);
    return resp.signatureShare;
  }

  override async getIdentityPublicKey(): Promise<Uint8Array> {
    const resp = await this.conn.request({ type: "GetIdentityPublicKey" });
    if (resp.type !== "PublicKey") throw new Error(`unexpected response: ${resp.type}`);
    return resp.publicKey;
  }

  override async getDepositSigningKey(): Promise<Uint8Array> {
    const resp = await this.conn.request({ type: "GetDepositPublicKey" });
    if (resp.type !== "PublicKey") throw new Error(`unexpected response: ${resp.type}`);
    return resp.publicKey;
  }

  override async signSchnorrWithIdentityKey(message: Uint8Array): Promise<Uint8Array> {
    const resp = await this.conn.request({ type: "SignSchnorrIdentity", message });
    if (resp.type !== "Signature") throw new Error(`unexpected response: ${resp.type}`);
    return resp.signature;
  }

  override async getPublicKeyFromDerivation(keyDerivation?: KeyDerivation): Promise<Uint8Array> {
    if (!keyDerivation) {
      throw new Error("BleHardwareSigner.getPublicKeyFromDerivation requires a keyDerivation");
    }
    switch (keyDerivation.type) {
      case KeyDerivationType.LEAF: {
        const resp = await this.conn.request({ type: "GetLeafPublicKey", leafId: keyDerivation.path });
        if (resp.type !== "PublicKey") throw new Error(`unexpected response: ${resp.type}`);
        return resp.publicKey;
      }
      case KeyDerivationType.DEPOSIT:
        return this.getDepositSigningKey();
      default:
        throw new Error(
          `BleHardwareSigner.getPublicKeyFromDerivation: ${keyDerivation.type} isn't wired into ble.rs yet (only LEAF/DEPOSIT are)`,
        );
    }
  }

  override async signMessageWithIdentityKey(message: Uint8Array, compact?: boolean): Promise<Uint8Array> {
    const resp = await this.conn.request({ type: "SignEcdsaIdentity", message, compact: compact ?? false });
    if (resp.type !== "Signature") throw new Error(`unexpected response: ${resp.type}`);
    return resp.signature;
  }

  override async subtractAndSplitSecretWithProofsGivenDerivations({
    first,
    second,
    threshold,
    numShares,
  }: Omit<SplitSecretWithProofsParams, "secret"> & {
    first: KeyDerivation;
    second: KeyDerivation | undefined;
  }): Promise<VerifiableSecretShare[]> {
    if (!second) {
      throw new Error("BleHardwareSigner.subtractAndSplitSecretWithProofsGivenDerivations requires `second`");
    }
    const resp = await this.conn.request({
      type: "SubtractAndSplitSecretWithProofs",
      first: toKeyDerivationRefWire(first),
      second: toKeyDerivationRefWire(second),
      threshold,
      numShares,
    });
    if (resp.type !== "Shares") throw new Error(`unexpected response: ${resp.type}`);
    return sharesFromWire(resp.shares);
  }

  // Despite decrypting to a private key internally, the interface contract
  // for this method returns only the *public* key of that decrypted value
  // (see `DefaultSparkSigner.decryptEcies` in signer.ts) -- needed on the
  // claim path specifically by `verifyPendingTransfer` (called from
  // `claimTransferCore` before the key-tweak step even runs). Confirmed
  // required on real hardware: without this override, claiming failed
  // with "identityKey not initialized" (the base class's fallback for
  // this exact method, since this signer never populates that field).
  override async decryptEcies(ciphertext: Uint8Array): Promise<Uint8Array> {
    const resp = await this.conn.request({ type: "DecryptEciesToPublicKey", ciphertext });
    if (resp.type !== "PublicKey") throw new Error(`unexpected response: ${resp.type}`);
    return resp.publicKey;
  }

  // Sending side of the leaf-ownership-transfer key tweak, needed
  // whenever a payment's leaves don't sum to the exact invoice amount and
  // the SSP swaps them for ones that do. Confirmed required on real
  // hardware: without this override, such a payment failed with "Private
  // key not initialized" (`DefaultSparkSigner`'s own
  // `subtractSplitAndEncrypt` falling through to
  // `getSigningPrivateKeyFromDerivation`, which needs local key material
  // this signer never has).
  override async subtractSplitAndEncrypt({
    first,
    second,
    threshold,
    numShares,
    receiverPublicKey,
  }: SubtractSplitAndEncryptParams): Promise<SubtractSplitAndEncryptResult> {
    const resp = await this.conn.request({
      type: "SubtractSplitAndEncrypt",
      first: toKeyDerivationRefWire(first),
      second: toKeyDerivationRefWire(second),
      receiverPublicKey,
      threshold,
      numShares,
    });
    if (resp.type !== "SubtractSplitAndEncrypt") throw new Error(`unexpected response: ${resp.type}`);
    return { shares: sharesFromWire(resp.shares), secretCipher: resp.secretCipher };
  }
}
