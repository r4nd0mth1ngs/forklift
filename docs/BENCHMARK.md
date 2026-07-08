# Benchmarking Forklift against Git

A runnable guide for comparing **Forklift** to **Git** on real, large repositories
(Git's own source, or the Linux kernel). It ships with a one-command harness and,
below that, the manual steps the harness automates — so you can reproduce every
number by hand and add your own.

> **Read this first — the honest framing.** Git is a 20-year-old C codebase tuned
> against the exact repos below; Forklift is **v0.1.x**. Expect Git to win some raw
> single-command timings and Forklift to reach parity or better on others (status,
> commit, and — on a packed store — the history walk and on-disk size). The point of
> benchmarking here is not to "beat git" — it
> is to (a) see where Forklift is already in the same ballpark, (b) find the
> operations that are *unexpectedly* slow so they become optimization targets, and
> (c) measure the things Forklift does that Git doesn't have a column for (signed
> history, per-directory inventory). Treat a result that's within a small multiple
> of git as a good outcome for a system this young.

---

## 1. What you need

| Requirement | Why | Check |
|-------------|-----|-------|
| `git` on PATH | the baseline, and the source of history to import | `git --version` |
| `forklift` on PATH | the system under test | `forklift version` |
| A timer: GNU `date`, `python3`, or `perl` | wall-clock measurement | any of `python3 --version` |
| Disk: ~2× the repo's size | the harness keeps two working copies | `df -h .` |
| *(optional)* [`hyperfine`](https://github.com/sharkdp/hyperfine) | tighter statistics, warmup, outlier detection | `hyperfine --version` |

Install hyperfine if you want publication-grade numbers: `brew install hyperfine`
(macOS) or `cargo install hyperfine`. The bundled harness does **not** require it.

Disk budget, concretely:
- **git.git** — ~230 MB of history, ~4.7k files. Two copies ≈ 0.6 GB. Fast.
- **torvalds/linux** — ~5 GB of history, ~90k files. Two copies + import ≈ **20+ GB**
  and a multi-minute import. Only run this when you specifically want kernel-scale.

---

## 2. Quick start — the one-command harness

From the repo root:

```sh
# Git's own source (the good default: large but not enormous)
bin/benchmark --repo git

# The Linux kernel (huge — read the disk note above first)
bin/benchmark --repo linux

# Any repo you like — a URL or a local path
bin/benchmark --repo https://github.com/rust-lang/cargo.git
bin/benchmark --repo /path/to/some/local/repo

# Save the results table to a file, and keep the working copies to poke at
bin/benchmark --repo git --out results.md --keep
```

Options: `--runs N` (iterations for the fast read-only ops, default 5),
`--work DIR` (where to build the copies; default a fresh temp dir),
`--keep` (don't delete the work dir), `--out FILE` (append the results table).

### What it measures

The harness fetches the target once, then builds two working copies of the **same
tree** so each comparison is apples-to-apples — `git` runs in copy `A`, `forklift`
in copy `B`:

| Operation | git | forklift |
|-----------|-----|----------|
| **status** (clean tree) | `git status` | `forklift stocktake --summary` |
| **log** (whole history) | `git log` | `forklift history` |
| **diff** (dirty tree) | `git diff` | `forklift diff --staged` |
| **commit** (1 file) | `git add` + `git commit` | `forklift load` + `forklift stack` |
| **onboard** *(separate)* | `git clone --local` | `forklift import-git .` |
| **compaction** *(payoff, separate)* | — | `forklift compact` — loose→packed before/after |

The forklift copy is **compacted right after import**, so the four comparison rows run
on the *packed* store — the same shape git is always measured in (`clone --local` copies
packfiles; a real `import-git` packs on the way out). The loose→packed win is reported
separately as a forklift-only payoff, never as a ratio against git.

Output is an **aligned text table** (pass `--markdown` for a pasteable markdown one):
git time, forklift time, the **forklift/git ratio** (below 1.0 means forklift was
faster), and a per-row note. Example shape:

```
  Repo:      git — 81470 commits, 4775 tracked files
  On-disk:   git .git = 316M   forklift .forklift = 236M  (forklift 1.3x smaller)

  Operation               git        forklift   forklift/git   Notes
  ----------------------  ---------  ---------  -------------  ----------------------------------
  status (clean tree)     172 ms     22 ms      0.13x          git status vs forklift stocktake …
  log (whole history)     929 ms     944 ms     1.02x          81470 commits walked
  diff (20 files changed) 25 ms      43 ms      1.72x          git diff vs forklift diff --staged
  commit (1 file)         41 ms      38 ms      0.93x          git add+commit vs load+stack

  Onboarding — measured separately; NOT a ratio (different operations):
    git clone --local     462 ms     (copies an existing packfile)
    forklift import-git   130.91 s   (re-encodes every commit/tree/blob)

  Compaction payoff (forklift's own before/after — no git equivalent):
  The table above runs packed; this is what packing bought over the loose store.
    Compacted 402118 loose objects into 5 packs, delta-compressed (236 MiB).
    on-disk:  .forklift 4.8G loose -> 236M packed
    log walk: 4.37 s loose -> 944 ms packed   (0.22x once packed)
    compacted in 22.38 s   (a real import-git folds this in automatically)
```

**Compaction runs *first*, right after import**, so the comparison table above runs on
the **packed store** — the state a real user operates in (git ships packed too:
`clone --local` copies packfiles, and a real `import-git` packs on the way out). Packed
vs packed, forklift lands smaller on disk than git's own pack and roughly at parity on
the whole-history walk. The loose→packed before/after is then reported on its own as a
**forklift-only payoff** — there is no git equivalent (git packs during `clone`/`gc`).
Packing removes per-file slack and the open-per-object cost of a walk, and
delta-compresses similar objects. See
[`OBJECT_STORE_SCALING.md`](OBJECT_STORE_SCALING.md). *(Numbers illustrative — run it.)*

**Onboarding is deliberately kept out of the ratio table.** `git clone --local` and
`forklift import-git` do fundamentally different work (see §3), so pitting them in a
ratio ("290× slower!") is misleading — the harness reports the two timings side by
side and lets you read them as what they are. *(Illustrative numbers — run it to get
your own.)*

---

## 3. Fairness caveats — read before you quote a number

These matter. A benchmark that hides them is a sales pitch, not a measurement.

- **Onboard is not a like-for-like race.** `git clone --local` hardlinks/copies an
  existing pack; `forklift import-git` *re-encodes* every git commit, tree and blob
  into Forklift objects and signs nothing (imported history is legacy/unsigned). So
  onboard measures the *one-time migration cost*, and git will look dramatically
  faster because it is doing dramatically less work. The honest question it answers
  is "how long to bring this history under Forklift", not "who clones faster".
  Two things to expect from `import-git` today:
  - **Import speed.** The importer streams every object through one long-lived
    `git cat-file --batch` pipe rather than forking git per object, so it stays fast
    at scale: all of git.git (~81k commits, ~402k objects → a 4.8 GB warehouse)
    imports in ~135 s. It still re-encodes and stores every object, so it is not
    instant like a local clone — expect a couple of minutes for kernel-scale history,
    not seconds. (Before this was batched it ran ~30× slower — one `git` fork+exec
    per object, ~67 min projected for the same import.)
  - **Non-UTF-8 history is tolerated.** Real repos carry commits with Latin-1 author
    names (git.git has several); the importer coerces such display text lossily
    rather than aborting. If you see U+FFFD (`�`) in an imported name, that's why —
    the author's email (the stable id) is preserved exactly.
  - **The table is measured packed, because git is.** By default `import-git` compacts
    the store on the way out, so a real user's warehouse is already packed — and git is
    always measured packed (`clone --local` copies packfiles). So the harness imports
    with `--no-compact`, captures the loose baseline for the payoff callout, then packs
    the store *before* the comparison table. Every `status`/`log`/`diff`/`commit` row is
    therefore packed-vs-packed. Measuring the read-only ops on the loose store instead
    would pit git-packed against forklift-loose — apples-to-oranges, and it understates
    forklift several-fold on `log` (a loose store is a transient that exists only in the
    seconds between an import and its auto-compaction). The loose→packed win is reported
    separately, not folded into the ratios.
- **`log` output differs.** `git log` and `forklift history` don't print identical
  text (history density, merge interleaving). The harness compares the *graph-walk
  cost* of the default command each ships, not byte-for-byte output. If you want a
  stricter comparison, pin both to a fixed format (e.g. `git log --format=%H`).
- **Warm vs cold cache.** The harness runs warm (the OS has already cached the tree
  from building the copies). First-run/cold numbers are a different, also-valid
  experiment — drop the caches between runs (`sync` + platform-specific cache drop)
  if that's what you care about.
- **`--summary` vs full status.** The harness uses `forklift stocktake --summary`
  (counts only) against `git status`. That's the closest fair pairing (neither
  formats a long per-file list); use plain `forklift stocktake` if you want the
  full-report cost instead.
- **One machine, few runs.** The bundled timer reports a mean over a handful of
  runs, not a distribution. For anything you plan to publish, re-run the same
  commands under `hyperfine` (see §5) — it does warmup, many runs, and outlier
  detection.
- **Forklift signs; git doesn't (by default).** If you `office enroll` the imported
  warehouse, every `stack` afterwards signs the parcel — real work git isn't doing
  in its `commit`. Benchmark signed vs unsigned deliberately; don't conflate them.

---

## 4. Doing it by hand

The harness just automates this. Run it yourself to understand or extend it.

```sh
# 0. Get a repo to test on, once.
git clone https://github.com/git/git.git ~/bench/git-src

# 1. Two working copies of the same tree.
git clone --local ~/bench/git-src ~/bench/A          # git measured here
cp -a ~/bench/A ~/bench/B                             # forklift measured here

# 2. Import history into the forklift copy (this is the 'onboard' number).
cd ~/bench/B
forklift prepare
time forklift import-git . --no-compact      # --no-compact so we can see the loose→packed win

# 2b. Pack the store BEFORE comparing — git is always packed, so this keeps it fair.
#     (A plain `import-git` without --no-compact does this for you.)
du -sh .forklift                             # loose baseline
time forklift compact
du -sh .forklift                             # packed — this is the state the table compares

# 3. Compare, running each tool in its own copy.
# status:
( cd ~/bench/A && time git status )
( cd ~/bench/B && time forklift stocktake --summary )

# log (whole-history walk):
( cd ~/bench/A && time git log            >/dev/null )
( cd ~/bench/B && time forklift history   >/dev/null )

# diff (dirty the same files in both, then diff):
for f in README.md Makefile; do printf '\n// edit\n' >> ~/bench/A/$f ~/bench/B/$f; done
( cd ~/bench/A && time git diff           >/dev/null )
( cd ~/bench/B && forklift load . && time forklift diff --staged >/dev/null )

# commit (stage a change, then commit/stack):
( cd ~/bench/A && echo x >> README.md && git add README.md   && time git commit -q -m bench )
( cd ~/bench/B && echo x >> README.md && forklift load README.md && time forklift stack bench )
```

### Extend it — ops the harness doesn't cover

Good candidates to add for a deeper picture:

- **Branch switch / tree materialization** — `git checkout <old-branch>` vs
  `forklift shift <pallet>`. Forklift rewrites the working tree and repopulates the
  inventory on a shift, so this exercises a very different code path than status.
  (Both refuse or overwrite based on a dirty tree — start clean.)
- **Branch create** — `git branch x` vs `forklift palletize x`.
- **Stash** — `git stash` / `git stash pop` vs `forklift park` / `forklift park pop`.
- **Signed history** — `forklift office enroll`, then time a signed `forklift stack`
  and an `forklift audit` (verifying the whole chain offline). Git has no direct
  equivalent; the closest is `git commit -S` + `git log --show-signature`.
- **Cold cache** — repeat any read-only op after dropping OS file caches.

---

## 5. Going deeper with hyperfine

For real statistics, run the same commands under hyperfine. It handles warmup,
many runs, and — crucially — a `--prepare` step for mutating commands:

```sh
# read-only, straightforward:
hyperfine --warmup 3 \
  --command-name 'git status'    'git -C ~/bench/A status' \
  --command-name 'forklift st'   'forklift --json stocktake --summary'   # run from ~/bench/B

# whole-history log:
hyperfine --warmup 2 \
  'git -C ~/bench/A log' \
  'sh -c "cd ~/bench/B && forklift history"'

# commit needs a fresh staged change each run — use --prepare:
hyperfine --prepare 'echo $RANDOM >> ~/bench/A/README.md && git -C ~/bench/A add README.md' \
  'git -C ~/bench/A commit -q -m bench'
```

Use `--export-markdown out.md` to capture a table, or `--export-json` for plots.

---

## 6. Interpreting results — what "good" looks like

- **Same order of magnitude as git** on status/diff/commit is a genuine win for a
  v0.1 system. A large multiple on a *fast* op (tens of ms) is often fixed overhead
  (process start, lock acquisition, inventory open) that amortizes away on big
  operations — note the absolute time, not just the ratio.
- **`log`/`history`** stresses graph-walk and object decode, measured on the **packed**
  store (where forklift is ~at parity with git). On a *loose* store it is several-fold
  slower — that's the object-store read-path cost compaction exists to remove, which is
  why the harness packs before measuring and reports the loose penalty as a payoff, not a
  row. A regression in the *packed* number is the real object-store read-path signal.
- **`onboard`** will always favor git (see §3). Track it over releases to catch
  import regressions, not to compare against clone.
- **A regression across releases matters more than the absolute gap to git.**
  Re-run `bin/benchmark --repo git --out history.md` after changes and diff the
  tables — that's the highest-signal use of this harness.

If a number looks wrong, re-run with `--keep` and inspect the two working copies
(`A/` = git, `B/` = forklift) by hand.
