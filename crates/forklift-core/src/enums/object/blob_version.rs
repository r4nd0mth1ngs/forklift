use crate::builder::object::blob::version;
use crate::model::blob::Blob;

const CODE_VERSION_2024_09_04: u64 = 1;

/// Versions of the blob object format.
pub enum BlobVersion {
    /// The original version, defined on 09/04/2024.
    V2024_09_04,
}

impl BlobVersion {
    /// Get the code of the version.
    ///
    /// # Returns
    /// * `u64` - The code of the version.
    pub fn get_code(&self) -> u64 {
        match self {
            BlobVersion::V2024_09_04 => CODE_VERSION_2024_09_04,
        }
    }

    /// Get the version for the given code.
    ///
    /// # Arguments
    /// * `code` - The code of the version.
    ///
    /// # Returns
    /// * `Ok(BlobVersion)` - The version associated with the given code.
    /// * `Err(String)`     - The error message if the version is unknown.
    pub fn from_code(code: u64) -> Result<BlobVersion, String> {
        match code {
            CODE_VERSION_2024_09_04 => Ok(BlobVersion::V2024_09_04),
            _ => Err(format!("Unknown blob version: {}", code)),
        }
    }

    /// Get the object builder function for the version.
    ///
    /// # Returns
    /// * `impl Fn(&Blob) -> Vec<u8>` - The object builder function.
    pub fn get_builder(&self) -> impl Fn(&Blob) -> Vec<u8> + '_ {
        let builder_fn = match self {
            BlobVersion::V2024_09_04 => version::v2024_09_04::build,
        };

        move |b: &Blob| builder_fn(self.get_code(), b)
    }

    /// Get the object parser function for the version.
    ///
    /// # Returns
    /// * `impl Fn(usize, &[u8]) -> Result<Blob, String>` - The object parser function.
    pub fn get_parser(&self) -> impl Fn(usize, &[u8]) -> Result<Blob, String> + '_ {
        let parser_fn = match self {
            BlobVersion::V2024_09_04 =>
                crate::parser::object::blob::version::v2024_09_04::parse,
        };

        move |offset, content| parser_fn(offset, content)
    }

    /// Get the latest version of the blob object format.
    ///
    /// # Returns
    /// The latest version of the blob object format.
    pub fn latest() -> BlobVersion {
        BlobVersion::V2024_09_04
    }
}