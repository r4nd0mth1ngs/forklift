use std::fmt::Display;
use std::str::FromStr;

const OBJECT_TYPE_BLOB: &str = "blob";
const OBJECT_TYPE_PARCEL: &str = "parcel";
const OBJECT_TYPE_TREE: &str = "tree";

const CODE_BLOB: u64 = 1;
const CODE_PARCEL: u64 = 2;
const CODE_TREE: u64 = 3;

/// Types of objects recognized by Forklift.
#[derive(PartialEq)]
pub enum ObjectType {
    Blob,
    Parcel,
    Tree,
}

impl FromStr for ObjectType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            OBJECT_TYPE_BLOB => Ok(ObjectType::Blob),
            OBJECT_TYPE_PARCEL => Ok(ObjectType::Parcel),
            OBJECT_TYPE_TREE => Ok(ObjectType::Tree),
            _ => Err(format!("Object type \"{}\" not found.", s)),
        }
    }
}

impl Display for ObjectType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let str = match self {
            ObjectType::Blob => OBJECT_TYPE_BLOB.to_string(),
            ObjectType::Parcel => OBJECT_TYPE_PARCEL.to_string(),
            ObjectType::Tree => OBJECT_TYPE_TREE.to_string(),
        };
        write!(f, "{}", str)
    }
}

impl ObjectType {
    /// Get the code of the object type.
    ///
    /// # Returns
    /// * `u64` - The code of the object type.
    pub fn get_code(&self) -> u64 {
        match self {
            ObjectType::Blob => CODE_BLOB,
            ObjectType::Parcel => CODE_PARCEL,
            ObjectType::Tree => CODE_TREE,
        }
    }

    /// Get the object type for the given code.
    ///
    /// # Arguments
    /// * `code` - The code of the object type.
    ///
    /// # Returns
    /// * `Ok(ObjectType)` - The object type associated with the given code.
    /// * `Err(String)`    - The error message if the object type is unknown.
    pub fn from_code(code: u64) -> Result<ObjectType, String> {
        match code {
            CODE_BLOB => Ok(ObjectType::Blob),
            CODE_PARCEL => Ok(ObjectType::Parcel),
            CODE_TREE => Ok(ObjectType::Tree),
            _ => Err(format!("Unknown object type: {}", code)),
        }
    }
}