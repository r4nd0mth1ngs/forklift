# Pallet refs

Pallets (git counterpart: branches) are mutable pointers to head parcels. They are the
only mutable state in a warehouse — everything else is immutable, content-addressed
objects.

## Ref files

Every pallet that has at least one parcel is a file under `.forklift/pallets/`:

```
.forklift/pallets/<name>
```

Pallet names may contain `/`, which maps to subfolders (e.g. the pallet
`feature/load-command` is stored at `.forklift/pallets/feature/load-command`). Because a
name is a file and its prefix would have to be a folder, a pallet named `a` and a pallet
named `a/b` cannot coexist — the file system enforces this, matching git's behavior.

### Name rules

A pallet name consists of `/`-separated components. Each component:

* consists of ASCII letters, digits, `.`, `_` and `-`,
* is not empty, and is not `.` or `..`,
* does not start with `-` (such a name would be indistinguishable from a flag).

### Content

The content of a ref file is the Blake3 hash of the head parcel in ASCII hex (64
characters), terminated by a single new line byte:

```
9028a15ad613bcd9853a3e780cfe3c78361b56ce95a2430484ba75ade5198cdc\n
```

Ref files are updated atomically (temp file + rename).

## The current pallet

The file `.forklift/pallet` holds the name of the current pallet (git counterpart: HEAD)
— the pallet that `stack` advances — terminated by a single new line byte:

```
main\n
```

A current pallet whose ref file does not exist yet is **unborn**: nothing has been
stacked on it. It is born when the first parcel is stacked. `prepare` sets the current
pallet to `main` without creating a ref file, so a freshly prepared warehouse has exactly
one unborn pallet.
