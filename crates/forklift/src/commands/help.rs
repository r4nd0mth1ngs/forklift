use clap::CommandFactory;
use crate::cli::Cli;

/// Handle the help command: print the command list, or the detailed help of one command
/// (following subcommands, so "help office admit" works too). This is the same help clap
/// renders for `--help`; the subcommand only exists to keep `forklift help <command>`
/// and the `h` alias working.
///
/// # Arguments
/// * `path` - The command (and subcommand) names to explain; empty for the command list.
///
/// # Returns
/// * `Ok(())`      - If the help was printed.
/// * `Err(String)` - If no command with the given name exists.
pub fn handle_command(path: &[String]) -> Result<(), String> {
    let mut command = Cli::command();
    let mut bin_name = command.get_name().to_string();

    for name in path {
        let subcommand = command
            .get_subcommands()
            .find(|sub| sub.get_name() == name || sub.get_all_aliases().any(|alias| alias == name))
            .cloned();

        command = subcommand.ok_or(format!(
            "Unknown command: {}. Use \"forklift help\" for a list of available commands.",
            path.join(" ")
        ))?;

        // A standalone subcommand does not know its parent chain; rebuilding the binary
        // name keeps the usage line correct (e.g. "forklift office admit ...").
        bin_name = format!("{} {}", bin_name, command.get_name());
        command = command.bin_name(bin_name.clone());
    }

    command.print_long_help()
        .map_err(|e| format!("Error while printing the help: {}", e))
}
