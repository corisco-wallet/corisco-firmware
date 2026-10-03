// Minimal postcard (https://postcard.jamesmunns.com) encoder/decoder,
// covering exactly the `Request`/`Response` shapes firmware-core's
// ble.rs speaks (see that file's `Request`/`Response` enums) -- not a
// general-purpose postcard implementation. Verified against postcard's
// own documented wire format spec (not guessed): unsigned integers use
// little-endian-group LEB128 varints (7 data bits/byte, MSB = more-bytes
// flag); strings/byte sequences are `varint(usize) length` + raw bytes;
// `Option<T>` is a `0x00`/`0x01` tag byte then T if present; enum
// variants are `varint(u32)` discriminant (declaration order) then the
// variant's fields in order, struct fields with no names/tags at all
// (postcard is not self-describing -- both ends already know the shape).

export class PostcardWriter {
  private bytes: number[] = [];

  writeVarintU32(n: number): void {
    let v = n >>> 0;
    while (true) {
      const byte = v & 0x7f;
      v >>>= 7;
      if (v === 0) {
        this.bytes.push(byte);
        break;
      }
      this.bytes.push(byte | 0x80);
    }
  }

  writeBool(b: boolean): void {
    this.bytes.push(b ? 1 : 0);
  }

  writeBytes(data: Uint8Array): void {
    this.writeVarintU32(data.length);
    for (let i = 0; i < data.length; i++) this.bytes.push(data[i]!);
  }

  writeString(s: string): void {
    this.writeBytes(new TextEncoder().encode(s));
  }

  writeOptionBytes(data: Uint8Array | undefined): void {
    if (data === undefined) {
      this.bytes.push(0);
    } else {
      this.bytes.push(1);
      this.writeBytes(data);
    }
  }

  // u64 varint via bigint arithmetic, not the 32-bit bitwise ops
  // writeVarintU32 uses -- JS's `>>>`/`&` truncate to 32 bits, which
  // would silently corrupt a sat amount above ~4.29 billion.
  writeVarintU64(n: bigint): void {
    let v = n;
    while (true) {
      const byte = Number(v & 0x7fn);
      v >>= 7n;
      if (v === 0n) {
        this.bytes.push(byte);
        break;
      }
      this.bytes.push(byte | 0x80);
    }
  }

  writeOptionU64(n: bigint | undefined): void {
    if (n === undefined) {
      this.bytes.push(0);
    } else {
      this.bytes.push(1);
      this.writeVarintU64(n);
    }
  }

  writeOptionString(s: string | undefined): void {
    if (s === undefined) {
      this.bytes.push(0);
    } else {
      this.bytes.push(1);
      this.writeString(s);
    }
  }

  finish(): Uint8Array {
    return new Uint8Array(this.bytes);
  }
}

export class PostcardReader {
  private pos = 0;
  constructor(private buf: Uint8Array) {}

  readVarintU32(): number {
    let result = 0;
    let shift = 0;
    while (true) {
      const byte = this.buf[this.pos++]!;
      result |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) break;
      shift += 7;
    }
    return result >>> 0;
  }

  readBool(): boolean {
    return this.buf[this.pos++]! !== 0;
  }

  readBytes(): Uint8Array {
    const len = this.readVarintU32();
    const out = this.buf.slice(this.pos, this.pos + len);
    this.pos += len;
    return out;
  }

  readString(): string {
    return new TextDecoder().decode(this.readBytes());
  }

  get remaining(): number {
    return this.buf.length - this.pos;
  }
}

// -- Request (mirrors ble.rs's `Request` enum, declaration order matters --
// it IS the wire discriminant) --

export type StatechainCommitmentWire = {
  identifier: Uint8Array;
  hiding: Uint8Array;
  binding: Uint8Array;
};

// Mirrors ble.rs's `KeyDerivationRef` enum -- declaration order (Leaf,
// Deposit, StaticDeposit, Ecies, Random) is the wire discriminant, same as
// every other enum here.
export type KeyDerivationRefWire =
  | { type: "Leaf"; leafId: string }
  | { type: "Deposit" }
  | { type: "StaticDeposit"; idx: number }
  | { type: "Ecies"; ciphertext: Uint8Array }
  | { type: "Random" };

function writeKeyDerivationRef(w: PostcardWriter, kd: KeyDerivationRefWire): void {
  switch (kd.type) {
    case "Leaf":
      w.writeVarintU32(0);
      w.writeString(kd.leafId);
      break;
    case "Deposit":
      w.writeVarintU32(1);
      break;
    case "StaticDeposit":
      w.writeVarintU32(2);
      w.writeVarintU32(kd.idx);
      break;
    case "Ecies":
      w.writeVarintU32(3);
      w.writeBytes(kd.ciphertext);
      break;
    case "Random":
      w.writeVarintU32(4);
      break;
  }
}

export type Request =
  | { type: "Commit" }
  | {
      type: "Sign";
      commitmentId: number;
      leafId: string;
      message: Uint8Array;
      statechainCommitments: StatechainCommitmentWire[];
      verifyingKey: Uint8Array;
      adaptorPublicKey: Uint8Array | undefined;
      requiresConfirmation: boolean;
      // App-asserted, not verified against `message` -- see ble.rs's
      // `Request::Sign` doc comment on why that can't be done against the
      // real SDK (it only ever hands a signer the already-hashed sighash,
      // never the raw transaction).
      amountSats: bigint | undefined;
      destination: string | undefined;
    }
  | { type: "GetIdentityPublicKey" }
  | { type: "GetDepositPublicKey" }
  | { type: "GetLeafPublicKey"; leafId: string }
  | { type: "SignSchnorrIdentity"; message: Uint8Array }
  | { type: "SignEcdsaIdentity"; message: Uint8Array; compact: boolean }
  | {
      type: "SubtractAndSplitSecretWithProofs";
      first: KeyDerivationRefWire;
      second: KeyDerivationRefWire;
      threshold: number;
      numShares: number;
    }
  | { type: "DecryptEciesToPublicKey"; ciphertext: Uint8Array }
  | {
      type: "SubtractSplitAndEncrypt";
      first: KeyDerivationRefWire;
      second: KeyDerivationRefWire;
      receiverPublicKey: Uint8Array;
      threshold: number;
      numShares: number;
    };

export function encodeRequest(req: Request): Uint8Array {
  const w = new PostcardWriter();
  switch (req.type) {
    case "Commit":
      w.writeVarintU32(0);
      break;
    case "Sign":
      w.writeVarintU32(1);
      w.writeVarintU32(req.commitmentId);
      w.writeString(req.leafId);
      w.writeBytes(req.message);
      w.writeVarintU32(req.statechainCommitments.length);
      for (const sc of req.statechainCommitments) {
        w.writeBytes(sc.identifier);
        w.writeBytes(sc.hiding);
        w.writeBytes(sc.binding);
      }
      w.writeBytes(req.verifyingKey);
      w.writeOptionBytes(req.adaptorPublicKey);
      w.writeBool(req.requiresConfirmation);
      w.writeOptionU64(req.amountSats);
      w.writeOptionString(req.destination);
      break;
    case "GetIdentityPublicKey":
      w.writeVarintU32(2);
      break;
    case "GetDepositPublicKey":
      w.writeVarintU32(3);
      break;
    case "GetLeafPublicKey":
      w.writeVarintU32(4);
      w.writeString(req.leafId);
      break;
    case "SignSchnorrIdentity":
      w.writeVarintU32(5);
      w.writeBytes(req.message);
      break;
    case "SignEcdsaIdentity":
      w.writeVarintU32(6);
      w.writeBytes(req.message);
      w.writeBool(req.compact);
      break;
    case "SubtractAndSplitSecretWithProofs":
      w.writeVarintU32(7);
      writeKeyDerivationRef(w, req.first);
      writeKeyDerivationRef(w, req.second);
      w.writeVarintU32(req.threshold);
      w.writeVarintU32(req.numShares);
      break;
    case "DecryptEciesToPublicKey":
      w.writeVarintU32(8);
      w.writeBytes(req.ciphertext);
      break;
    case "SubtractSplitAndEncrypt":
      w.writeVarintU32(9);
      writeKeyDerivationRef(w, req.first);
      writeKeyDerivationRef(w, req.second);
      w.writeBytes(req.receiverPublicKey);
      w.writeVarintU32(req.threshold);
      w.writeVarintU32(req.numShares);
      break;
  }
  return w.finish();
}

// -- Response (mirrors ble.rs's `Response` enum) --

// Mirrors ble.rs's `ShareWire` struct (wire format for
// `signer_core::vss::VerifiableSecretShare`).
export type ShareWire = {
  threshold: number;
  index: number;
  share: Uint8Array;
  proofs: Uint8Array[];
};

function readShareWire(r: PostcardReader): ShareWire {
  const threshold = r.readVarintU32();
  const index = r.readVarintU32();
  const share = r.readBytes();
  const proofsLen = r.readVarintU32();
  const proofs: Uint8Array[] = [];
  for (let i = 0; i < proofsLen; i++) proofs.push(r.readBytes());
  return { threshold, index, share, proofs };
}

export type Response =
  | { type: "Commit"; commitmentId: number; hiding: Uint8Array; binding: Uint8Array }
  | { type: "Sign"; signatureShare: Uint8Array }
  | { type: "PublicKey"; publicKey: Uint8Array }
  | { type: "Signature"; signature: Uint8Array }
  | { type: "Error"; message: string }
  | { type: "Shares"; shares: ShareWire[] }
  | { type: "SubtractSplitAndEncrypt"; shares: ShareWire[]; secretCipher: Uint8Array };

export function decodeResponse(bytes: Uint8Array): Response {
  const r = new PostcardReader(bytes);
  const variant = r.readVarintU32();
  switch (variant) {
    case 0:
      return { type: "Commit", commitmentId: r.readVarintU32(), hiding: r.readBytes(), binding: r.readBytes() };
    case 1:
      return { type: "Sign", signatureShare: r.readBytes() };
    case 2:
      return { type: "PublicKey", publicKey: r.readBytes() };
    case 3:
      return { type: "Signature", signature: r.readBytes() };
    case 4:
      return { type: "Error", message: r.readString() };
    case 5: {
      const len = r.readVarintU32();
      const shares: ShareWire[] = [];
      for (let i = 0; i < len; i++) shares.push(readShareWire(r));
      return { type: "Shares", shares };
    }
    case 6: {
      const len = r.readVarintU32();
      const shares: ShareWire[] = [];
      for (let i = 0; i < len; i++) shares.push(readShareWire(r));
      return { type: "SubtractSplitAndEncrypt", shares, secretCipher: r.readBytes() };
    }
    default:
      throw new Error(`decodeResponse: unknown Response variant ${variant}`);
  }
}
