# Blob object format
This format is used to store blobs in the objects store.

## Structure
Below is the structure of the blob object (as of version `V2024_09_04`).
Each `[...]` represents a byte or a sequence of bytes.
```
[object_format_version_vlq][blob_data]
```
Where:
- `object_format_version_vlq` is the code of the blob object format version, stored
as a variable-length quantity. See the list of version codes [here](../codes/BLOB_OBJECT_FORMAT_VERSION_CODES.md).
- `blob_data` is the raw data of the blob.