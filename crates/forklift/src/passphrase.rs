//! The terminal side of passphrase-protected keys: the CLI head owns all interaction
//! with the operator, so it installs the provider that `forklift-core` calls when it
//! needs to unlock a protected key, and it prompts for a new passphrase when one is
//! being set. Core never touches a terminal itself.

use forklift_core::util::sign_utils;

/// Install the passphrase provider core uses to unlock protected keys: a terminal
/// prompt. In an unattended context (no controlling terminal — an agent's
/// non-interactive subprocess) the prompt fails, so a protected key cannot be
/// unlocked there. Called once, from `main`.
pub fn install_provider() {
    sign_utils::set_passphrase_provider(Box::new(|key_id: &str| {
        rpassword::prompt_password(format!("Passphrase for key {}: ", key_id))
            .map_err(|_| format!(
                "Key {} is passphrase-protected and no terminal is available to ask for \
                its passphrase — this key cannot be used non-interactively.",
                key_id
            ))
    }));
}

/// Prompt for a new key's passphrase, confirmed twice so a typo does not lock the key
/// away. Honors `FORKLIFT_KEY_PASSPHRASE` (automation and tests) before prompting.
///
/// # Returns
/// * `Ok(String)`  - The chosen passphrase.
/// * `Err(String)` - If the terminal is unavailable, the passphrase is empty, or the
///                   two entries do not match.
pub fn prompt_new() -> Result<String, String> {
    if let Ok(passphrase) = std::env::var(sign_utils::ENV_KEY_PASSPHRASE) {
        if !passphrase.is_empty() {
            return Ok(passphrase);
        }
    }

    let passphrase = rpassword::prompt_password("Choose a passphrase for the new key: ")
        .map_err(|_| "No terminal is available to read a passphrase.".to_string())?;

    if passphrase.is_empty() {
        return Err("The passphrase must not be empty.".to_string());
    }

    let confirmation = rpassword::prompt_password("Confirm the passphrase: ")
        .map_err(|_| "No terminal is available to read a passphrase.".to_string())?;

    if passphrase != confirmation {
        return Err("The passphrases do not match.".to_string());
    }

    Ok(passphrase)
}
