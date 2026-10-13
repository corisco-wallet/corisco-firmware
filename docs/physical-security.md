# Physical security

What protects the seed if someone gets the board in their hands.

## Device-bound seed (implemented)

The seed is encrypted with a key derived from the PIN **and** a secret computed by
the chip's HMAC peripheral from an eFuse key block. The key block is
read-protected, so software and an SPI flash dump never see it. A dump of the
flash alone cannot be brute-forced offline, whatever the PIN length.

Boards without the key block (development boards) fall back to PIN-only
encryption. The first unlock after a key is provisioned re-seals the seed with
the device secret.

### Provisioning (irreversible, one key block per board)

```bash
head -c 32 /dev/urandom > hmac.bin
espefuse.py --chip esp32s3 -p <PORT> burn_key BLOCK_KEY0 hmac.bin HMAC_UP
shred -u hmac.bin
```

`burn_key` read- and write-protects the block. Try it on a sacrificial board
first. Use any unused key block; the firmware finds the one with the
`HMAC_UP` purpose.

Consequences:

- The seed on that board can only be unlocked by that chip. The mnemonic backup
  is the only recovery path if the chip dies.
- Flashing firmware to a board that holds a bound seed but has no key block
  shows "Device key missing" and does not count as a wrong PIN.

## Not done yet

- **Secure boot v2 and flash encryption.** Without them anyone with physical
  access can replace the firmware, for example with one that reads the seed
  after you type the PIN. They need a larger bootloader and a moved partition
  table, so they must be rehearsed on a sacrificial board; none of it has been
  tested here.
- **Authenticated PIN-failure counter.** The counter lives in plain NVS. Someone
  with flash access can reset it and keep guessing on the real device. The
  device-bound key stops offline guessing; online guessing is limited only by the
  KDF time per attempt, so a longer PIN is the remaining lever. Encrypting NVS
  would not stop a rollback to an older copy either; only an eFuse-backed
  monotonic counter would.

## Out of scope

There is no secure element. An attacker with lab equipment (fault injection,
decapping) is not covered.
