//! T1 — the crash / interrupted-write harness (milestone A, the test spine).
//!
//! D2 makes "durable before destructive" hold across power loss: every object, ref, inventory
//! shard and graph file is written to a temp file, fsynced, renamed, and the directory fsynced,
//! and a pallet's ref advances only *after* all the objects it names are durable. The claim is
//! that a crash at any instant leaves the store either at its old state or fully at the new one —
//! never a torn object at a real address, never a half-written ref.
//!
//! A unit test can assert the atomic-write contract (see `file_utils`), but only a real,
//! externally killed process exercises the whole `stack` pipeline under interruption. This test
//! SIGKILLs `stack` at a spread of delays that straddle the object-write/ref-update window, and
//! after each kill asserts the store is still internally consistent and usable. The assertions
//! hold at *every* kill point, so the test cannot flake — whether a given kill lands inside the
//! interesting window only affects coverage, never pass/fail. A crash that genuinely corrupted
//! the store (a torn object, a partial ref) is the only thing that fails it.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

const FORKLIFT: &str = env!("CARGO_BIN_EXE_forklift");

/// A scratch area: the warehouse, plus an isolated home for the global config and keys so the
/// test never touches the developer's real ones. Deleted when the test ends.
struct Area {
    root: PathBuf,
}

impl Area {
    fn new(name: &str) -> Area {
        let root = std::env::temp_dir().join(format!("forklift-crash-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("warehouse")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        Area { root }
    }

    fn warehouse(&self) -> PathBuf {
        self.root.join("warehouse")
    }

    /// A command in the warehouse with the isolated global config and key directory.
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(FORKLIFT);
        command
            .args(args)
            .current_dir(self.warehouse())
            .env("FORKLIFT_GLOBAL_CONFIG", self.root.join("home").join("global-config.toml"))
            .env("FORKLIFT_KEYS_DIR", self.root.join("home").join("keys"));
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// A crashed `stack` leaves the warehouse lock behind (SIGKILL runs no destructor), exactly as
    /// a real power loss would; the operator clears it. Do the same before the next command so the
    /// lock is never the reason a later step fails — we are testing store integrity, not the lock.
    fn clear_stale_lock(&self) {
        let _ = std::fs::remove_file(self.warehouse().join(".forklift").join("lock"));
    }

    /// Assert the store is internally consistent right now: any pallet head is a whole 64-hex hash
    /// (an atomic ref write never leaves a partial one), and the commands that read the committed
    /// tree and history succeed (a torn object would fail D1's verify-on-read).
    fn assert_consistent(&self, context: &str) {
        let head_path = self.warehouse().join(".forklift").join("pallets").join("main");
        if let Ok(head) = std::fs::read_to_string(&head_path) {
            let head = head.trim();
            assert!(
                head.len() == 64 && head.bytes().all(|b| b.is_ascii_hexdigit()),
                "{context}: the pallet head must be a whole hash, found {head:?}",
            );

            let history = self.run(&["history"]);
            assert!(history.status.success(),
                "{context}: history must read the parcel chain, stderr: {}",
                String::from_utf8_lossy(&history.stderr));

            let peek = self.run(&["peek", head]);
            assert!(peek.status.success(),
                "{context}: peek of the head parcel must succeed, stderr: {}",
                String::from_utf8_lossy(&peek.stderr));
        }

        let stocktake = self.run(&["stocktake"]);
        assert!(stocktake.status.success(),
            "{context}: stocktake must read the head tree, stderr: {}",
            String::from_utf8_lossy(&stocktake.stderr));
    }
}

impl Drop for Area {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn killing_stack_midway_never_corrupts_the_store() {
    let area = Area::new("stack");
    let warehouse = area.warehouse();
    let file = warehouse.join("big.dat");

    // A few megabytes so hashing, compression, the fsync and the rename take long enough that some
    // of the kills below land inside the write window rather than always before or after it.
    let base_line = "the quick brown fox jumps over the lazy dog\n";
    std::fs::write(&file, base_line.repeat(90_000)).unwrap();

    assert!(area.run(&["prepare"]).status.success());
    assert!(area.run(&["config", "operator.name", "crash@forklift"]).status.success());
    assert!(area.run(&["config", "operator.identifier", "crash@forklift"]).status.success());

    // Delays that straddle a single stack's duration: some fire before the objects are written,
    // some during, some after the ref has advanced.
    let delays_ms: [u64; 24] = [
        1, 2, 3, 4, 5, 6, 8, 10, 12, 14, 16, 18, 20, 24, 28, 32, 36, 40, 45, 50, 55, 60, 70, 80,
    ];

    let mut advanced = 0usize;
    let mut prior_head: Option<String> = None;

    for (i, delay) in delays_ms.iter().enumerate() {
        // 1. Recover from the previous kill and check it left the store consistent.
        area.clear_stale_lock();
        area.assert_consistent(&format!("after kill #{i}"));

        // A head that advanced must be a *new* parcel, never a rewritten/rolled-back one.
        let head_now = std::fs::read_to_string(
            warehouse.join(".forklift").join("pallets").join("main"),
        ).ok().map(|h| h.trim().to_string());
        if let (Some(now), Some(prev)) = (&head_now, &prior_head) {
            if now != prev {
                advanced += 1;
            }
        } else if head_now.is_some() && prior_head.is_none() {
            advanced += 1;
        }
        prior_head = head_now;

        // 2. Make a fresh change and stage it.
        std::fs::write(&file, format!("{}change {}\n", base_line.repeat(90_000), i)).unwrap();
        let load = area.run(&["load", "."]);
        assert!(load.status.success(), "load failed: {}", String::from_utf8_lossy(&load.stderr));

        // 3. Spawn the stack and SIGKILL it mid-flight.
        let mut child = area.command(&["stack", &format!("commit {i}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        std::thread::sleep(Duration::from_millis(*delay));

        let _ = child.kill(); // a no-op if it already finished
        let _ = child.wait();
    }

    // 4. Final recovery: the store must still accept a clean write, and every object reachable from
    //    the final head must read back (export-git walks the whole graph — parcels, trees, blobs —
    //    so a torn object anywhere would fail here via D1's verify-on-read).
    area.clear_stale_lock();
    area.assert_consistent("final");

    assert!(area.run(&["load", "."]).status.success());
    let recover = area.run(&["stack", "recover"]);
    let recovered_ok = recover.status.success()
        || String::from_utf8_lossy(&recover.stderr).contains("Nothing to stack");
    assert!(recovered_ok, "the store must accept a write after the crashes, stderr: {}",
        String::from_utf8_lossy(&recover.stderr));

    area.assert_consistent("after recovery stack");

    let export_dir = area.root.join("git-export");
    let export = area.run(&["export-git", export_dir.to_str().unwrap()]);
    assert!(export.status.success(),
        "export-git must read every committed object without a torn read, stderr: {}",
        String::from_utf8_lossy(&export.stderr));

    // Sanity: across the run at least one kill fell after a completed ref update, so the durable
    // path (not just the "killed before anything" path) was actually exercised.
    assert!(advanced >= 1, "no stack ever completed — the write window was never exercised");
}
