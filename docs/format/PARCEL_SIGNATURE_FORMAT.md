# Parcel signature format
Once trust is established for a warehouse (see `TRACKED_METADATA.md`), every parcel is
signed. The signature covers the **full parcel hash** — never selected fields — so it
transitively covers the tree (all content), the parents (all history) and every metadata
byte. Signing selected fields would create malleability bugs; this rule is recorded in
the design document (§6, FORK-13 review).

The signature lives in a **sidecar file next to the parcel object**, never inside it:
the parcel hash covers the full uncompressed object (design invariant §4.4), so a
signature over that hash cannot be part of the hashed content. The sidecar is named like
the object file, plus the `.sig` suffix:

```
.forklift/objects/<first-2-hash-chars>/<remaining-hash-chars>.sig
```

## Structure (V1, latest)
Each `[...]` represents a byte or a sequence of bytes.
```
[format_version_vlq][key_id_length_vlq][key_id][signature_length_vlq][signature]
```
Where:
- `format_version_vlq` is the signature format version (`1`), stored as a
variable-length quantity.
- `key_id_length_vlq` is the length of the key id in bytes, stored as a variable-length
quantity.
- `key_id` is the id of the signing key (as ASCII hex bytes): the Blake3 hash of the raw
public key bytes. The public key itself is tracked in the office records (see
`TRACKED_METADATA.md`).
- `signature_length_vlq` is the length of the signature in bytes, stored as a
variable-length quantity (64 for Ed25519).
- `signature` is the Ed25519 signature over the parcel hash's ASCII bytes.

## Verification
A parcel verifies when its sidecar's signature verifies against the public key of
`key_id` as tracked in the office records. The `audit` command additionally checks the
office chain itself: every office parcel must be signed by a key that was active in the
*previous* office state — introducing a key and signing with it in the same parcel is
only valid for the genesis parcel (that self-signature is the trust-on-first-use anchor).
