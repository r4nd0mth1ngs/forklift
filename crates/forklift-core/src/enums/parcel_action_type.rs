use serde::Serialize;

const CODE_AUTHOR: u64 = 1;
const CODE_STACK: u64 = 2;

/// A type of interaction between an operator and a parcel.
#[derive(Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ParcelActionType {
    // Contributed changes.
    Author,
    // Stacked the parcel to the pallet.
    Stack,
}

impl ParcelActionType {
    /// Get the code of the parcel action type.
    ///
    /// # Returns
    /// * `u64` - The code of the parcel action type.
    pub fn get_code(&self) -> u64 {
        match self {
            ParcelActionType::Author => CODE_AUTHOR,
            ParcelActionType::Stack => CODE_STACK,
        }
    }

    /// Get the parcel action type for the given code.
    ///
    /// # Arguments
    /// * `code` - The code of the parcel action type.
    ///
    /// # Returns
    /// * `Ok(ParcelActionType)` - The parcel action type associated with the given code.
    /// * `Err(String)`          - The error message if the parcel action type is unknown.
    pub fn from_code(code: u64) -> Result<ParcelActionType, String> {
        match code {
            CODE_AUTHOR => Ok(ParcelActionType::Author),
            CODE_STACK => Ok(ParcelActionType::Stack),
            _ => Err(format!("Unknown parcel action type: {}", code)),
        }
    }

    /// Get the name of the parcel action type for peeking.
    /// The name may have some padding at the end to make sure that all names have the same length.
    ///
    /// # Returns
    /// * `String` - The name of the parcel action type.
    pub fn get_name_for_peek(&self) -> String {
        match self {
            ParcelActionType::Author => "author",
            ParcelActionType::Stack =>  "stack ",
        }.to_string()
    }
}