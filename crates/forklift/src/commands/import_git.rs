use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use chrono::{TimeZone, Utc};
use serde::Serialize;
use forklift_core::builder::object::loose_object_builder::LooseObjectBuilder;
use forklift_core::enums::dir_entry_type::DirEntryType;
use forklift_core::enums::parcel_action_type::ParcelActionType;
use forklift_core::model::blob::Blob;
use forklift_core::model::operator::Operator;
use forklift_core::model::parcel::Parcel;
use forklift_core::model::parcel_action::ParcelAction;
use forklift_core::model::tree_item::TreeItem;
use forklift_core::util::{inventory_utils, object_utils, office_utils, pallet_utils, shift_utils};
use crate::output::{self, CommandOutput};

/// Handle the import-git command (§7.8): one-way migration of a git repository's history
/// into this warehouse. Git commits become parcels, trees become trees, blobs become
/// blobs, and each local branch becomes a pallet — so a project (or an agent moving one)
/// can adopt forklift without leaving its history behind.
///
/// The imported history is **unsigned** (it predates any trust): import into a warehouse
/// where trust is not yet established, then `office enroll` to anchor it as the legacy
/// boundary. Run it from the git repository's directory (`forklift import-git .`), so the
/// checked-out working tree already matches the imported HEAD.
///
/// A large import lands hundreds of thousands of loose objects at once, which is exactly
/// the case the object store is slowest and largest in until it is packed. So import packs
/// the store itself on the way out (`--no-compact` opts out) — the user gets a dense
/// warehouse without having to remember to run `compact`.
///
/// # Arguments
/// * `path`       - The path of the git repository to import from.
/// * `no_compact` - Skip the automatic post-import compaction (leave the store loose).
///
/// # Returns
/// * `Ok(())`      - If the import completed.
/// * `Err(String)` - If `path` is not a git repository, trust is already established, a
///   pallet would collide, or git could not be read.
pub fn handle_command(path: &str, no_compact: bool) -> Result<(), String> {
    // Imported parcels are unsigned; a trusted warehouse rejects those, so import must
    // come first — enrolling afterwards records the imported heads as the legacy boundary.
    if office_utils::read_trust_anchor()?.is_some() {
        return Err(
            "Trust is already established here; unsigned git history cannot be imported. \
            Import into a fresh warehouse, then \"office enroll\".".to_string()
        );
    }

    // Validate it is a git repository up front (a clear error beats a cryptic one later).
    git(path, &["rev-parse", "--git-dir"])?;

    let branches = branches(path)?;

    if branches.is_empty() {
        return Err(format!("\"{}\" has no local branches to import.", path));
    }

    let mut converter = Converter::new(path)?;

    // Convert every commit oldest-first (so each commit's parents are already mapped),
    // pulling in the trees and blobs each one references on the way.
    for commit in all_commits(path)? {
        converter.convert_commit(&commit)?;
    }

    // Each local branch becomes a pallet.
    let mut imported: Vec<(String, String)> = Vec::new();

    for (branch, commit) in &branches {
        let pallet = sanitize_pallet_name(branch);

        if pallet_utils::does_pallet_exist(&pallet) {
            return Err(format!(
                "Pallet \"{}\" already exists; import into a fresh warehouse.", pallet
            ));
        }

        let head = converter.commits.get(commit).cloned().ok_or(format!(
            "Branch \"{}\" points at a commit that was not imported.", branch
        ))?;

        pallet_utils::set_pallet_head(&pallet, &head)?;
        imported.push((branch.clone(), pallet));
    }

    // Check out git's HEAD branch (or the first branch) as the current pallet, and build
    // the inventory from it so a colocated working tree reads clean.
    let default_branch = default_branch(path).ok();
    let current = default_branch.as_ref()
        .and_then(|branch| imported.iter().find(|(name, _)| name == branch))
        .or_else(|| imported.first())
        .map(|(_, pallet)| pallet.clone());

    if let Some(pallet) = &current {
        pallet_utils::set_current_pallet_name(pallet)?;

        if let Some(head) = pallet_utils::get_pallet_head(pallet)? {
            let tree = object_utils::load_parcel(&head)?.tree_hash;
            let shards = shift_utils::build_inventories_for_tree(&tree)?;
            inventory_utils::replace_all_inventories(&shards)?;
        }
    }

    // A colocated git repo keeps its `.git` folder in the working tree; forklift should
    // never track it. Adding the pattern is harmless when the repo lives elsewhere.
    let ignored_git = ignore_git_directory()?;

    // Pack the freshly-imported loose objects into the object store, unless opted out. Done
    // last, so the inventory build above still reads objects from the fast loose path; the
    // import already holds the warehouse lock, so this runs inside it. A compaction failure
    // must not fail the (successful) import — the store is simply left loose to `compact`
    // later — so it is reported as a warning rather than propagated.
    let compacted = if no_compact {
        None
    } else {
        match forklift_core::util::pack_utils::compact(false) {
            Ok(stats) => Some(Compaction { objects: stats.objects_packed, packs: stats.packs_written }),
            Err(error) => {
                converter.warnings.push(format!(
                    "the import succeeded but compaction did not ({}); run \"forklift compact\" to pack the store", error
                ));
                None
            }
        }
    };

    output::emit("import-git", &ImportReport {
        commits: converter.commits.len(),
        trees: converter.trees.len(),
        blobs: converter.blobs.len(),
        pallets: imported.iter().map(|(_, pallet)| pallet.clone()).collect(),
        current,
        ignored_git,
        compacted,
        warnings: converter.warnings,
    });

    Ok(())
}

/// Add a `.git` ignore pattern to `.forkliftignore` if it is not already there, so a
/// colocated git repository's metadata folder is not treated as untracked content.
fn ignore_git_directory() -> Result<bool, String> {
    let path = forklift_core::globals::warehouse_root().join(".forkliftignore");
    let pattern = r"^\.git\/?.*$";

    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    if existing.lines().any(|line| line.trim() == pattern) {
        return Ok(false);
    }

    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str("# git metadata (added by import-git)\n");
    content.push_str(pattern);
    content.push('\n');

    std::fs::write(&path, content)
        .map_err(|e| format!("Error while updating the ignore file: {}", e))?;

    Ok(true)
}

/// A long-lived `git cat-file --batch` process. Every commit, tree and blob is read through
/// this one streaming pipe instead of a fresh `git` fork+exec per object — the dominant cost
/// of importing a large history (git.git is ~80k commits and several hundred thousand
/// objects, i.e. that many process spawns the old path paid one at a time).
struct GitBatch {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl GitBatch {
    fn new(path: &str) -> Result<GitBatch, String> {
        let mut child = Command::new("git")
            .arg("-C").arg(path)
            .args(["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("Could not run git (is it installed and on your PATH?): {}", e))?;

        let stdin = child.stdin.take().ok_or("git cat-file: no stdin pipe".to_string())?;
        let stdout = BufReader::new(child.stdout.take().ok_or("git cat-file: no stdout pipe".to_string())?);

        Ok(GitBatch { child, stdin, stdout })
    }

    /// Fetch one object by hash: its git type (`commit`/`tree`/`blob`) and its raw bytes.
    /// The batch protocol answers each request line with a `<sha> <type> <size>\n` header,
    /// then exactly `<size>` content bytes, then a trailing newline.
    fn read(&mut self, hash: &str) -> Result<(String, Vec<u8>), String> {
        self.stdin.write_all(hash.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(|e| format!("git cat-file: could not request {}: {}", hash, e))?;

        let mut header = String::new();
        self.stdout.read_line(&mut header)
            .map_err(|e| format!("git cat-file: could not read the header for {}: {}", hash, e))?;
        let header = header.trim_end();

        let mut parts = header.split(' ');
        let _sha = parts.next();
        let kind = parts.next()
            .ok_or_else(|| format!("git cat-file: malformed header \"{}\"", header))?;
        if kind == "missing" {
            return Err(format!("git cat-file: object {} is missing", hash));
        }
        let size: usize = parts.next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| format!("git cat-file: no size in header \"{}\"", header))?;

        let mut content = vec![0u8; size];
        self.stdout.read_exact(&mut content)
            .map_err(|e| format!("git cat-file: short read for {}: {}", hash, e))?;

        let mut terminator = [0u8; 1];
        self.stdout.read_exact(&mut terminator)
            .map_err(|e| format!("git cat-file: missing record terminator for {}: {}", hash, e))?;

        Ok((kind.to_string(), content))
    }
}

impl Drop for GitBatch {
    fn drop(&mut self) {
        // The helper is disposable. stdin is still open here, so waiting on the child would
        // block it on EOF — kill it, then reap so we never leave a zombie behind.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Converts git objects to forklift objects, memoizing by git hash so every object is read
/// (and stored) once, even when branches share history.
struct Converter {
    batch: GitBatch,
    oid_len: usize,
    blobs: HashMap<String, String>,
    trees: HashMap<String, String>,
    commits: HashMap<String, String>,
    warnings: Vec<String>,
}

impl Converter {
    fn new(path: &str) -> Result<Converter, String> {
        // Tree objects embed each child's hash as raw bytes, so we need the repo's hash
        // width (20 for sha1, git's default; 32 for a sha256 repo) to parse them.
        let oid_len = match git_str(path, &["rev-parse", "--show-object-format"])?.trim() {
            "sha256" => 32,
            _ => 20,
        };

        Ok(Converter {
            batch: GitBatch::new(path)?,
            oid_len,
            blobs: HashMap::new(),
            trees: HashMap::new(),
            commits: HashMap::new(),
            warnings: Vec::new(),
        })
    }

    /// Convert one git commit (its parents must already be converted).
    fn convert_commit(&mut self, git_hash: &str) -> Result<String, String> {
        if let Some(hash) = self.commits.get(git_hash) {
            return Ok(hash.clone());
        }

        let (_, bytes) = self.batch.read(git_hash)?;
        let commit = parse_commit(&String::from_utf8_lossy(&bytes))?;

        let mut parents: Vec<String> = Vec::new();
        for parent in &commit.parents {
            parents.push(self.convert_commit(parent)?);
        }

        let tree_hash = self.convert_tree(&commit.tree)?;

        // Git's author/committer split maps onto forklift's Author/Stack actions exactly.
        let parcel = Parcel {
            tree_hash,
            parents,
            actions: vec![
                ParcelAction {
                    operator: commit.author,
                    action: ParcelActionType::Author,
                    description: None,
                    timestamp: Utc.timestamp_opt(commit.author_time, 0).single().unwrap_or_else(Utc::now),
                },
                ParcelAction {
                    operator: commit.committer,
                    action: ParcelActionType::Stack,
                    description: None,
                    timestamp: Utc.timestamp_opt(commit.committer_time, 0).single().unwrap_or_else(Utc::now),
                },
            ],
            description: (!commit.message.is_empty()).then_some(commit.message),
        };

        let mut object = LooseObjectBuilder::build_parcel(&parcel);
        object.store()?;

        self.commits.insert(git_hash.to_string(), object.hash.clone());

        Ok(object.hash)
    }

    /// Convert one git tree recursively.
    fn convert_tree(&mut self, git_hash: &str) -> Result<String, String> {
        if let Some(hash) = self.trees.get(git_hash) {
            return Ok(hash.clone());
        }

        let mut tree = TreeItem::new(String::new(), String::new(), DirEntryType::Tree);

        let (_, bytes) = self.batch.read(git_hash)?;
        for entry in parse_raw_tree(&bytes, self.oid_len)? {
            let child = match entry.mode.as_str() {
                "40000" | "040000" => {
                    let hash = self.convert_tree(&entry.hash)?;
                    TreeItem::new(entry.name, hash, DirEntryType::Tree)
                }
                "160000" => {
                    // A submodule (gitlink) has no content to import — record it and skip.
                    self.warnings.push(format!("skipped submodule \"{}\" (git gitlinks are not imported)", entry.name));
                    continue;
                }
                mode => {
                    let item_type = match mode {
                        "100755" => DirEntryType::Executable,
                        "120000" => DirEntryType::SymbolicLink,
                        _ => DirEntryType::Normal,
                    };
                    let hash = self.convert_blob(&entry.hash)?;
                    TreeItem::new(entry.name, hash, item_type)
                }
            };

            tree.add_child(child);
        }

        let mut object = LooseObjectBuilder::build_tree(&tree);
        object.store()?;

        self.trees.insert(git_hash.to_string(), object.hash.clone());

        Ok(object.hash)
    }

    /// Convert one git blob.
    fn convert_blob(&mut self, git_hash: &str) -> Result<String, String> {
        if let Some(hash) = self.blobs.get(git_hash) {
            return Ok(hash.clone());
        }

        let (_, content) = self.batch.read(git_hash)?;
        let blob = Blob { content };

        let mut object = LooseObjectBuilder::build_blob(&blob);
        object.store()?;

        self.blobs.insert(git_hash.to_string(), object.hash.clone());

        Ok(object.hash)
    }
}

/// A parsed git commit.
struct GitCommit {
    tree: String,
    parents: Vec<String>,
    author: Operator,
    author_time: i64,
    committer: Operator,
    committer_time: i64,
    message: String,
}

/// One git tree entry.
struct GitTreeEntry {
    mode: String,
    hash: String,
    name: String,
}

/// Run `git -C <path> <args>` and return its stdout bytes.
fn git(path: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("git")
        .arg("-C").arg(path)
        .args(args)
        .output()
        .map_err(|e| format!("Could not run git (is it installed and on your PATH?): {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(output.stdout)
}

/// Run a git command and return its stdout as a string, coercing lossily.
///
/// Git object text — author/committer names, messages, tree entry names — is a byte
/// string with no UTF-8 guarantee: git.git and the Linux kernel both carry commits whose
/// author names are Latin-1 (e.g. "David_Kågedal", byte 0xE5), which strict decoding would
/// reject, aborting the whole import. Forklift's model already stores these fields as UTF-8
/// strings, so coerce lossily (invalid bytes become U+FFFD) rather than failing. The stable
/// identifier — the author's email — is ASCII and survives intact; only non-authoritative
/// display bytes are approximated. Hashes and refnames (the other callers) are ASCII, so
/// lossy decoding is a no-op for them. (Blob *content* never goes through here — it is read
/// as raw bytes by `git()`, so binary files import byte-exact.)
fn git_str(path: &str, args: &[&str]) -> Result<String, String> {
    Ok(String::from_utf8_lossy(&git(path, args)?).into_owned())
}

/// The local branches, as `(branch name, commit hash)`.
fn branches(path: &str) -> Result<Vec<(String, String)>, String> {
    let text = git_str(path, &["for-each-ref", "--format=%(objectname) %(refname:short)", "refs/heads/"])?;

    Ok(text.lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(commit, branch)| (branch.to_string(), commit.to_string()))
        .collect())
}

/// Git's HEAD branch name (the default checkout).
fn default_branch(path: &str) -> Result<String, String> {
    Ok(git_str(path, &["symbolic-ref", "--short", "HEAD"])?.trim().to_string())
}

/// Every commit reachable from the local branches, oldest first (so parents precede
/// children — the order the converter relies on).
fn all_commits(path: &str) -> Result<Vec<String>, String> {
    let text = git_str(path, &["rev-list", "--topo-order", "--reverse", "--branches"])?;

    Ok(text.lines().map(|line| line.trim().to_string()).filter(|line| !line.is_empty()).collect())
}

/// Parse the output of `git cat-file commit <hash>`.
fn parse_commit(text: &str) -> Result<GitCommit, String> {
    let (header, message) = text.split_once("\n\n").unwrap_or((text, ""));

    let mut tree = String::new();
    let mut parents: Vec<String> = Vec::new();
    let mut author: Option<(Operator, i64)> = None;
    let mut committer: Option<(Operator, i64)> = None;

    for line in header.lines() {
        if let Some(rest) = line.strip_prefix("tree ") {
            tree = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("parent ") {
            parents.push(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("author ") {
            author = Some(parse_identity(rest)?);
        } else if let Some(rest) = line.strip_prefix("committer ") {
            committer = Some(parse_identity(rest)?);
        }
        // Other headers (gpgsig and its continuation lines, encoding, …) are ignored.
    }

    if tree.is_empty() {
        return Err("A git commit has no tree.".to_string());
    }

    let (author, author_time) = author.ok_or("A git commit has no author.".to_string())?;
    let (committer, committer_time) = committer.unwrap_or_else(|| (author.clone(), author_time));

    Ok(GitCommit {
        tree,
        parents,
        author,
        author_time,
        committer,
        committer_time,
        message: message.trim_end_matches('\n').to_string(),
    })
}

/// Parse a git identity line: `Name <email> <unix-seconds> <tz-offset>`.
fn parse_identity(text: &str) -> Result<(Operator, i64), String> {
    let email_start = text.find(" <").ok_or("A git identity has no email.".to_string())?;
    let name = text[..email_start].trim().to_string();

    let after_lt = email_start + 2;
    let email_len = text[after_lt..].find('>').ok_or("A git identity has an unterminated email.".to_string())?;
    let email = text[after_lt..after_lt + email_len].to_string();

    let time = text[after_lt + email_len + 1..]
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or("A git identity has no timestamp.".to_string())?;

    // The email is the stable, pseudonymous id; the name is local display data.
    Ok((Operator { name, identifier: email }, time))
}

/// Parse a raw git tree object. `git cat-file --batch` hands back the object's own bytes
/// (not the text `ls-tree` prints): a packed sequence of `<octal-mode> SP <name> NUL
/// <oid-bytes>`, where the child hash is raw binary (`oid_len` bytes) and must be hex-encoded
/// to match the string form the rest of the importer keys on.
fn parse_raw_tree(bytes: &[u8], oid_len: usize) -> Result<Vec<GitTreeEntry>, String> {
    let mut entries = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let space = i + bytes[i..].iter().position(|&b| b == b' ')
            .ok_or("A git tree entry has no mode.".to_string())?;
        let mode = String::from_utf8_lossy(&bytes[i..space]).into_owned();

        let name_start = space + 1;
        let nul = name_start + bytes[name_start..].iter().position(|&b| b == 0)
            .ok_or("A git tree entry has an unterminated name.".to_string())?;
        let name = String::from_utf8_lossy(&bytes[name_start..nul]).into_owned();

        let oid_start = nul + 1;
        let oid_end = oid_start + oid_len;
        if oid_end > bytes.len() {
            return Err("A git tree entry has a truncated hash.".to_string());
        }
        let hash = hex_encode(&bytes[oid_start..oid_end]);

        entries.push(GitTreeEntry { mode, hash, name });
        i = oid_end;
    }

    Ok(entries)
}

/// Lowercase hex, matching git's textual object-id form.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Turn a git branch name into a valid forklift pallet name, replacing characters forklift
/// does not allow (it keeps `/`, so `feature/x` survives).
fn sanitize_pallet_name(branch: &str) -> String {
    let sanitized: String = branch.chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/') { c } else { '-' })
        .collect();

    // A leading '-' (or an empty result) would be rejected; anchor it.
    if sanitized.is_empty() || sanitized.starts_with('-') {
        format!("branch-{}", sanitized.trim_start_matches('-'))
    } else {
        sanitized
    }
}

/// The result of a git import.
#[derive(Serialize)]
struct ImportReport {
    commits: usize,
    trees: usize,
    blobs: usize,

    /// The pallets created (one per local branch).
    pallets: Vec<String>,

    /// The pallet checked out (git's HEAD branch).
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<String>,

    /// Whether a `.git` ignore pattern was added to `.forkliftignore`.
    ignored_git: bool,

    /// The automatic post-import compaction, when it ran (absent with `--no-compact`).
    #[serde(skip_serializing_if = "Option::is_none")]
    compacted: Option<Compaction>,

    /// Anything skipped or worth flagging (e.g. submodules).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

/// The post-import compaction summary (a slim view of `pack_utils::CompactStats`).
#[derive(Serialize)]
struct Compaction {
    /// Loose objects packed.
    objects: usize,

    /// Packs written.
    packs: usize,
}

impl CommandOutput for ImportReport {
    fn render_human(&self) {
        println!(
            "Imported {} commit(s) into {} pallet(s) ({} tree(s), {} blob(s)).",
            self.commits, self.pallets.len(), self.trees, self.blobs
        );

        if let Some(current) = &self.current {
            println!("Checked out \"{}\".", current);
        }

        for warning in &self.warnings {
            println!("  warning: {}", warning);
        }

        if self.ignored_git {
            println!("Added \".git/\" to \".forkliftignore\".");
        }

        if let Some(compaction) = &self.compacted {
            if compaction.objects > 0 {
                println!(
                    "Packed the imported store: {} object(s) into {} pack(s).",
                    compaction.objects, compaction.packs
                );
            }
        }

        println!(
            "The history is unsigned (imported). Run \"office enroll\" to establish trust — \
            it anchors the imported history as the legacy boundary."
        );
    }
}
