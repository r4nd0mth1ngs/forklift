//! Generates the reference fragments under `docs/generated/` (errors table, per-command
//! `--json` schemas). Only compiled in behind the `docgen` feature (see `bin/gen-docs`); a
//! release build never enables it, so this module — and the `schemars` dependency it needs —
//! costs the shipped binary nothing.
//!
//! Both renderers are deterministic (fixed iteration order), so `bin/gen-docs` run twice
//! produces byte-identical output and `bin/check` can verify freshness with a plain diff.

use crate::cli::Cli;
use crate::output::ErrorCode;
use clap::CommandFactory;

/// Render `docs/generated/errors.md`: the exhaustive error-code / exit-code reference.
pub fn render_errors() -> String {
    let mut out = String::new();
    out.push_str(GENERATED_BANNER);
    out.push_str("# Error codes and exit codes\n\n");
    out.push_str(
        "Every `forklift` failure carries a stable `code` (in the `--json` error envelope) \
         and the process exits with the matching deterministic status, so a script or an agent \
         branches without parsing prose. `2` is reserved for clap's own argument/usage errors; \
         `0` is success. Both tables are generated from the single `ErrorCode` enum in \
         `crates/forklift/src/output.rs` — see `docs/guide/cli.md` for how a script is meant to \
         use them.\n\n",
    );
    out.push_str("| `code` | exit | Meaning |\n");
    out.push_str("|---|---|---|\n");
    for code in ErrorCode::ALL {
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            code.as_str(),
            code.exit_code(),
            code.description(),
        ));
    }
    out.push('\n');
    out
}

/// Render `docs/generated/json-schemas.md`: every CLI command's `--json` `data` payload
/// schema(s), derived from the `#[derive(schemars::JsonSchema)]` output structs in
/// `crates/forklift/src/commands/`.
pub fn render_json_schemas() -> String {
    let mut out = String::new();
    out.push_str(GENERATED_BANNER);
    out.push_str("# `--json` output schemas\n\n");
    out.push_str(
        "Every command's `--json` result is `{ \"forklift_json\", \"command\", \"ok\": true, \
         \"data\": … }` on success (see `docs/MACHINE_INTERFACE.md` for the envelope and the \
         failure shape). This page is the exhaustive reference for each command's `data` — one \
         [JSON Schema](https://json-schema.org/) per shape a command can emit; a command with \
         more than one (e.g. one per subcommand) lists all of them. Descriptions come straight \
         from the Rust doc comments on the underlying struct, so they stay in sync with the \
         field they describe.\n\n\
         A command not listed here (or listed with no schema below) either reports only the \
         generic human-message shape `{ \"message\": string }`, or produces no `--json` data at \
         all — see the command's entry in `docs/guide/cli.md`.\n\n",
    );

    let cli = Cli::command();
    for command in cli.get_subcommands() {
        let name = command.get_name();
        if name.starts_with("__") {
            continue; // hidden dev-only diagnostics (this generator itself), not a CLI command
        }

        let Some(schemas) = command_schemas(name) else { continue };
        if schemas.is_empty() {
            continue;
        }

        out.push_str(&format!("## `{}`\n\n", name));
        for (struct_name, schema) in schemas {
            out.push_str(&format!("### `{}`\n\n", struct_name));
            let pretty = serde_json::to_string_pretty(schema.as_value())
                .unwrap_or_else(|_| "{}".to_string());
            out.push_str("```json\n");
            out.push_str(&pretty);
            out.push_str("\n```\n\n");
        }
    }

    out
}

const GENERATED_BANNER: &str = "<!--\n\
    GENERATED FILE — do not edit by hand.\n\
    Produced by `bin/gen-docs` (crates/forklift/src/docgen.rs). `bin/check` regenerates and\n\
    diffs this file to catch drift; run `bin/gen-docs` and commit the result after a change\n\
    that affects it (a new error code, or a `--json` output struct).\n\
-->\n\n";

/// Look up the schema-registry function for a CLI command name, if it has one wired.
/// `None` (rather than an empty registry entry) also covers commands not derived at all —
/// the renderer treats both the same way (falls back to the generic-shape note).
fn command_schemas(name: &str) -> Option<Vec<(&'static str, schemars::Schema)>> {
    use crate::commands;

    Some(match name {
        "alias" => commands::alias::__docgen_schemas(),
        "audit" => commands::audit::__docgen_schemas(),
        "bay" => commands::bay::__docgen_schemas(),
        "blame" => commands::blame::__docgen_schemas(),
        "cherry-pick" => commands::cherry_pick::__docgen_schemas(),
        "compact" => commands::compact::__docgen_schemas(),
        "config" => commands::config::__docgen_schemas(),
        "conflicts" => commands::conflicts::__docgen_schemas(),
        "consolidate" => commands::consolidate::__docgen_schemas(),
        "deliver" => commands::deliver::__docgen_schemas(),
        "diff" => commands::diff::__docgen_schemas(),
        "expand" => commands::expand::__docgen_schemas(),
        "export-git" => commands::export_git::__docgen_schemas(),
        "franchise" => commands::franchise::__docgen_schemas(),
        "haul" => commands::haul::__docgen_schemas(),
        "history" => commands::history::__docgen_schemas(),
        "import-git" => commands::import_git::__docgen_schemas(),
        "lift" => commands::lift::__docgen_schemas(),
        "load" => commands::load::__docgen_schemas(),
        "lower" => commands::lower::__docgen_schemas(),
        "manifest" => commands::manifest::__docgen_schemas(),
        "narrow" => commands::narrow::__docgen_schemas(),
        "office" => commands::office::__docgen_schemas(),
        "palletize" => commands::palletize::__docgen_schemas(),
        "park" => commands::park::__docgen_schemas(),
        "peek" => commands::peek::__docgen_schemas(),
        "prepare" => commands::prepare::__docgen_schemas(),
        "profile" => commands::profile::__docgen_schemas(),
        "remove" => commands::remove::__docgen_schemas(),
        "scope" => commands::scope::__docgen_schemas(),
        "scope-prune" => commands::scope_prune::__docgen_schemas(),
        "self-update" => commands::self_update::__docgen_schemas(),
        "shift" => commands::shift::__docgen_schemas(),
        "stack" => commands::stack::__docgen_schemas(),
        "stocktake" => commands::stocktake::__docgen_schemas(),
        "store" => commands::store::__docgen_schemas(),
        "tag" => commands::tag::__docgen_schemas(),
        "undo" => commands::undo::__docgen_schemas(),
        "version" => commands::version::__docgen_schemas(),
        _ => return None,
    })
}
