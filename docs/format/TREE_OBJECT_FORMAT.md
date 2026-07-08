# Tree object format
This format is used to store tree objects in the objects store.

## Structure (V2026_07_02, latest)
Below is the structure of the tree object (as of version `V2026_07_02`).
Each `[...]` represents a byte or a sequence of bytes.
```
[object_format_version_vlq]
[entry_n_type_vlq][entry_n_name_length_vlq][entry_n_name][entry_n_hash][NL]
```
Where:
- `object_format_version_vlq` is the code of the tree object format version, stored
as a variable-length quantity. See the list of version codes [here](../codes/TREE_OBJECT_FORMAT_VERSION_CODES.md).
- `entry_n_type_vlq` is the code of the entry type, stored as a variable-length quantity.
A tree object can have multiple entries. Subtrees are listed first.
See the list of item type codes [here](../codes/DIR_ENTRY_TYPE_CODES.md).
- `entry_n_name_length_vlq` is the length of the entry name in bytes, stored as a
variable-length quantity. The name is length-prefixed (instead of terminated by a special
byte) because file names may legally contain any byte, including new line and EOT.
- `entry_n_name` is the name of the entry (as UTF-8 bytes).
- `entry_n_hash` is the hash of the object associated with the entry (as ASCII bytes).
- `NL` is an ASCII newline character (decimal value `10`). There is a newline byte after
every entry (including the last one). This is safe as a terminator because hashes are
ASCII hex and can never contain a newline byte.

## Structure (V2024_09_04)
Below is the structure of the tree object (as of version `V2024_09_04`).
Each `[...]` represents a byte or a sequence of bytes.
```
[object_format_version_vlq]
[entry_n_type_vlq][entry_n_name][EOT][entry_n_hash][NL]
```
Where:
- `object_format_version_vlq` is the code of the tree object format version, stored
as a variable-length quantity. See the list of version codes [here](../codes/TREE_OBJECT_FORMAT_VERSION_CODES.md).
- `entry_n_type_vlq` is the code of the entry type, stored as a variable-length quantity.
A tree object can have multiple entries, separated by newline bytes. Subtrees are listed first.
See the list of item type codes [here](../codes/DIR_ENTRY_TYPE_CODES.md).
- `entry_n_name` is the name of the entry (as UTF-8 bytes).
- `EOT` is an ASCII end-of-text character (decimal value `3`). It usually indicates the end
of a string. Note that this version cannot represent names that contain EOT or newline
bytes — this is why it was superseded by `V2026_07_02`.
- `entry_n_hash` is the hash of the object associated with the entry (as ASCII bytes).
- `NL` is an ASCII newline character (decimal value `10`). Entries are separated by newline bytes.
There is also a newline byte after the last entry.
