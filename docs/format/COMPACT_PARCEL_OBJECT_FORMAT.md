# Compact parcel format
This format is used to store parcels in the objects store.

## Authorship convention
Every parcel records its author(s) as explicit `AUTHOR` actions, even when the author and
the stacker are the same operator (like git, which records author == committer on plain
commits) — consumers can always rely on `AUTHOR` actions being present and never need a
fallback rule. Operations that create a parcel from another parcel's changes (cherry-pick
equivalents, hauls applied by a service) must preserve the source parcel's `AUTHOR`
actions and add their own `STACK` action.

The action list only holds actions knowable at parcel creation time: the parcel is hashed
and immutable, so anything that happens afterwards (reviews, approvals, third-party
signatures) belongs to post-metadata (FORK-9), never in here.

## Structure (V2026_07_02, latest)
Below is the structure of the parcel object (as of version `V2026_07_02`).
Each `[...]` represents a byte or a sequence of bytes.
```
[object_format_version_vlq][tree_hash][NL]
[parent_n_hash][NL]
[NULL]
[action_n_type_vlq][timestamp_vlq][operator_length_vlq][operator_identifier][description_length_vlq][description]
[NULL]
[parcel_description]
```
Where:
- `object_format_version_vlq` is the code of the compact parcel object format version, stored
as a variable-length quantity.
See the list of version codes [here](../codes/COMPACT_PARCEL_OBJECT_FORMAT_VERSION_CODES.md).
- `tree_hash` is the hash of the tree object associated with the parcel (as ASCII bytes).
- `NL` is an ASCII newline character (decimal value `10`). It is safe as a terminator for
hashes, because hashes are ASCII hex and can never contain a newline byte.
- `parent_n_hash` is the hash of a parent parcel (as ASCII bytes). A parcel can have multiple
parents, so there can be multiple entries in this section (separated by `NL`).
A `NULL` (zero) byte indicates the end of the "parents" section.
- `NULL` is a null (zero) byte. It indicates the end of a section. It cannot be confused with
action content: after an action ends, the parser expects either an action type code (which is
never zero) or this byte.
- `action_n_type_vlq` is the code of the action type, stored as a variable-length quantity.
See the list of action type codes [here](../codes/PARCEL_ACTION_TYPE_CODES.md).
- `timestamp_vlq` is the UNIX timestamp of the action (in seconds),
stored as a variable-length quantity.
- `operator_length_vlq` is the length of the operator identifier in bytes, stored as a
variable-length quantity.
- `operator_identifier` is the identifier of the operator (i.e. user) who performed the action,
stored as UTF-8 bytes. It is length-prefixed because it is user-controlled and may contain
any byte.
- `description_length_vlq` is the length of the action description in bytes, stored as a
variable-length quantity. A length of `0` means the action has no description.
- `description` is the description of the action (as UTF-8 bytes). It is length-prefixed
because descriptions may contain any byte (including new lines).
- `parcel_description` is the description of the parcel (as UTF-8 bytes). It is the remainder
of the object; a parcel without a description ends after the `NULL` byte of the actions
section.

## Structure (V2024_09_04)
Below is the structure of the parcel object (as of version `V2024_09_04`).
Each `[...]` represents a byte or a sequence of bytes.
A byte (sequence) wrapped in `(...)` means that it is optional.
```
[object_format_version_vlq][tree_hash][NL]
[parent_n_hash][NL]
[NULL]
[action_n_type_vlq][timestamp_vlq][operator_identifier][EOT]([description])[NL]
[NULL]
[parcel_description]
```
Where:
- `object_format_version_vlq` is the code of the compact parcel object format version, stored
as a variable-length quantity.
See the list of version codes [here](../codes/COMPACT_PARCEL_OBJECT_FORMAT_VERSION_CODES.md).
- `tree_hash` is the hash of the tree object associated with the parcel (as ASCII bytes).
- `NL` is an ASCII newline character (decimal value `10`).
- `parent_n_hash` is the hash of a parent parcel (as ASCII bytes). A parcel can have multiple
parents, so there can be multiple entries in this section (separated by `NL`).
A `NULL` (zero) byte indicates the end of the "parents" section.
- `NULL` is a null (zero) byte. It usually indicates the end of a section.
- `action_n_type_vlq` is the code of the action type, stored as a variable-length quantity.
See the list of action type codes [here](../codes/PARCEL_ACTION_TYPE_CODES.md).
- `timestamp_vlq` is the UNIX timestamp of the action (in seconds),
stored as a variable-length quantity.
- `operator_identifier` is the identifier of the operator (i.e. user) who performed the action,
stored as ASCII bytes.
- `EOT` is an ASCII end-of-text character (decimal value `3`). It usually indicates the end
of a string. Note that this version cannot represent descriptions that contain newline
bytes — this is why it was superseded by `V2026_07_02`.
- `description` is an **optional** description of the action (as UTF-8 bytes).
- `parcel_description` is the description of the parcel (as UTF-8 bytes).
