use serde::Serialize;
use forklift_core::util::{audit_utils, office_utils, pallet_utils};
use forklift_core::util::pallet_utils::{PalletNamespace, PalletRef};
use crate::output::{self, CommandOutput};

/// Handle the audit command: verify the warehouse's signed history offline.
///
/// * The office chain is verified first, from the genesis parcel forward: every office
///   parcel must be signed by a key that was active in the *previous* office state
///   (the genesis is self-signed by the key it introduces — the TOFU anchor).
/// * Then the given pallet (the current one by default) is verified: every parcel must
///   carry a valid signature by a tracked key. Parcels stacked before trust was
///   established are reported as legacy (unsigned) but do not fail the audit.
///
/// The verification itself lives in `forklift_core::util::audit_utils` — the reference
/// server runs the same checks before committing a ref update.
///
/// # Arguments
/// * `pallet` - The pallet to audit (`None` audits the current pallet).
///
/// # Returns
/// * `Ok(())`      - If the audit passed.
/// * `Err(String)` - If trust is not established, or any verification failed.
pub fn handle_command(pallet: Option<String>) -> Result<(), String> {
    let Some(anchor) = office_utils::read_trust_anchor()? else {
        return Err(
            "Trust is not established for this warehouse; there are no signatures to \
            audit. Establish it with \"office enroll\".".to_string()
        );
    };

    // A bare name is a working (user) pallet; `@office` reaches the office meta pallet.
    let pallet_ref = match pallet {
        Some(name) => PalletRef::parse(&name)?,
        None => PalletRef::user(pallet_utils::get_current_pallet_name()?),
    };

    let Some(office_head) = pallet_utils::get_meta_pallet_head(office_utils::OFFICE_PALLET_NAME)? else {
        return Err("Trust is established but the office pallet is missing.".to_string());
    };

    let office_state = audit_utils::verify_office_chain(&anchor, &office_head)?;

    let mut report = AuditReport {
        genesis: anchor.genesis.clone(),
        pallet: pallet_ref.to_wire(),
        pallet_verified: None,
        verified_parcels: 0,
        legacy_parcels: 0,
    };

    // Auditing the office pallet is just the office-chain verification above; any other
    // pallet gets its signed history verified against the office state too.
    let is_office = pallet_ref.namespace == PalletNamespace::Meta
        && pallet_ref.name == office_utils::OFFICE_PALLET_NAME;

    if !is_office {
        let Some(head) = pallet_utils::get_pallet_head_in(pallet_ref.namespace, &pallet_ref.name)? else {
            return Err(format!("No pallet named \"{}\" exists (or it has nothing stacked).", pallet_ref.to_wire()));
        };

        let (verified, legacy) = audit_utils::verify_pallet_history(&head, &anchor, &office_state, None)?;

        report.pallet_verified = Some(true);
        report.verified_parcels = verified;
        report.legacy_parcels = legacy;
    }

    output::emit("audit", &report);

    Ok(())
}

/// The result of an offline audit: the office chain always, plus a working pallet's
/// parcel counts when one (not the office) was audited.
#[derive(Serialize)]
struct AuditReport {
    /// The genesis the office chain verified back to.
    genesis: String,

    /// The pallet that was audited.
    pallet: String,

    /// Whether a working pallet's history was audited (absent when only the office
    /// chain was — i.e. the audited pallet *is* the office).
    #[serde(skip_serializing_if = "Option::is_none")]
    pallet_verified: Option<bool>,

    /// How many signed parcels verified on the pallet.
    verified_parcels: usize,

    /// How many legacy (pre-trust, unsigned) parcels were tolerated.
    legacy_parcels: usize,
}

impl CommandOutput for AuditReport {
    fn render_human(&self) {
        println!("Office chain verified back to genesis {}.", self.genesis);

        if self.pallet_verified != Some(true) {
            return;
        }

        println!(
            "Pallet \"{}\" verified: {} signed parcel(s) valid{}.",
            self.pallet,
            self.verified_parcels,
            if self.legacy_parcels > 0 {
                format!(", {} legacy parcel(s) predate trust and are unsigned", self.legacy_parcels)
            } else {
                String::new()
            }
        );
    }
}
