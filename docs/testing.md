# Spark regtest test wallets

Two ready-made regtest wallets for exercising a real end-to-end payment
against this app + a real device, without needing to fund/manage your own
test wallets first. These hold **REGTEST funds only, which have zero real
value** -- fine to keep in the repo, but keep the mnemonics out of any
real/mainnet context regardless.

Created with `npx @buildonspark/cli` (`@buildonspark/cli`, Node 20+).

## sender (funded)

- Mnemonic: `butter champion buzz cute robot gallery airport around thumb step width just`
- Network: REGTEST
- Static deposit address (L1 BTC, reusable): `bcrt1p49c5x2ecpn3ze8j3nztjvmrztufaupfu5xc9p6scyr07kvs50lgsnaum4s`
- Funded via: https://app.lightspark.com/regtest-faucet (paste the deposit address above)

Note: L1 deposits to the static deposit address don't auto-credit the Spark
balance. Two-step claim flow: `getlatesttx <depositAddress>` to find the
txid, then `claimstaticdepositwithmaxfee <txid> <maxFeeSats>` to actually
credit it.

To reuse this wallet in a CLI session:
```bash
npx -y @buildonspark/cli --network regtest \
  --mnemonic "butter champion buzz cute robot gallery airport around thumb step width just" \
  --exec getbalance
```

To test a real send from the hardware signer: pair `corisco-android-app` with a
provisioned device (see `esp32-lilygo-t-display-s3-firmware/README.md`), create an invoice
from the `receiver` wallet below (`createinvoice <amount> <memo> false
false`), and pay it from the app -- confirming on the device's own screen
completes the payment.

## receiver (unfunded)

The "other side" of a test payment -- a plain CLI wallet with the SDK's
own default (software) signer, no hardware involved. Used to create
invoices for the sender (or the app) to pay, and to confirm a payment
actually landed somewhere real.

- Mnemonic: `genius come risk dish phone lift scout museum toss pair siren spice`
- Network: REGTEST
- Spark address: `sparkrt1pgss8wj2utymdvv8362j3wp9qdwgm6v2mmf2ze8gg4dcg0kx58r709ct2u4twd`

To reuse this wallet in a CLI session:
```bash
npx -y @buildonspark/cli --network regtest \
  --mnemonic "genius come risk dish phone lift scout museum toss pair siren spice" \
  --exec getbalance
```
