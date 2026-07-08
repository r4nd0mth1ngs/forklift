use crate::builder::object::parcel::version;
use crate::model::parcel::Parcel;
use crate::parser;

const CODE_VERSION_2024_09_04: u64 = 1;
const CODE_VERSION_2026_07_02: u64 = 2;

/// Versions of the compact parcel format.
pub enum CompactParcelVersion {
    /// The original version, defined on 09/04/2024.
    V2024_09_04,

    /// The operator identifier and action description are length-prefixed instead of
    /// EOT / new line terminated, so that values containing those bytes cannot corrupt
    /// the object. Defined on 07/02/2026.
    V2026_07_02,
}

impl CompactParcelVersion {
    /// Get the code of the version.
    ///
    /// # Returns
    /// * `u64` - The code of the version.
    pub fn get_code(&self) -> u64 {
        match self {
            CompactParcelVersion::V2024_09_04 => CODE_VERSION_2024_09_04,
            CompactParcelVersion::V2026_07_02 => CODE_VERSION_2026_07_02,
        }
    }

    /// Get the version for the given code.
    ///
    /// # Arguments
    /// * `code` - The code of the version.
    ///
    /// # Returns
    /// * `Ok(CompactParcelVersion)` - The version associated with the given code.
    /// * `Err(String)`              - The error message if the version is unknown.
    pub fn from_code(code: u64) -> Result<CompactParcelVersion, String> {
        match code {
            CODE_VERSION_2024_09_04 => Ok(CompactParcelVersion::V2024_09_04),
            CODE_VERSION_2026_07_02 => Ok(CompactParcelVersion::V2026_07_02),
            _ => Err(format!("Unknown compact parcel version: {}", code)),
        }
    }

    /// Get the object builder function for the version.
    ///
    /// # Returns
    /// * `impl Fn(&Parcel) -> Vec<u8>` - The object builder function.
    pub fn get_builder(&self) -> impl Fn(&Parcel) -> Vec<u8> + '_ {
        let builder_fn = match self {
            CompactParcelVersion::V2024_09_04 => version::v2024_09_04::build,
            CompactParcelVersion::V2026_07_02 => version::v2026_07_02::build,
        };

        move |p: &Parcel| builder_fn(self.get_code(), p)
    }

    /// Get the object parser function for the version.
    ///
    /// # Returns
    /// * `impl Fn(usize, &[u8]) -> Result<Parcel, String>` - The object parser function.
    pub fn get_parser(&self) -> impl Fn(usize, &[u8]) -> Result<Parcel, String> + '_ {
        let parser_fn = match self {
            CompactParcelVersion::V2024_09_04 =>
                parser::object::parcel::version::v2024_09_04::parse,
            CompactParcelVersion::V2026_07_02 =>
                parser::object::parcel::version::v2026_07_02::parse,
        };

        move |offset, content| parser_fn(offset, content)
    }

    /// Get the latest version of the compact parcel format.
    ///
    /// # Returns
    /// The latest version of the compact parcel format.
    pub fn latest() -> CompactParcelVersion {
        CompactParcelVersion::V2026_07_02
    }
}