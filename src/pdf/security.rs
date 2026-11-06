//! PDF password protection. PDFium can open encrypted files but cannot write
//! encryption, so protection is applied to PDFium's saved output with lopdf
//! (AES-256, PDF 2.0 revision 6). See ADR 0006.

use std::collections::BTreeMap;
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{Document, EncryptionState, EncryptionVersion, Permissions};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Protection {
    /// Required to open the document. Empty means anyone can open it.
    pub open_password: String,
    /// Required to change permissions. Defaults to the open password when empty.
    pub owner_password: String,
    pub allow_printing: bool,
    pub allow_copying: bool,
    pub allow_editing: bool,
}

/// Encrypts an unencrypted PDF byte stream with AES-256.
pub fn encrypt(pdf: &[u8], p: &Protection) -> Result<Vec<u8>, String> {
    if p.open_password.is_empty() && p.owner_password.is_empty() {
        return Err("A password is required.".into());
    }
    let mut doc = Document::load_mem(pdf).map_err(|e| format!("could not parse the document for encryption: {e}"))?;
    if doc.is_encrypted() {
        return Err("The document is already encrypted.".into());
    }
    let mut key = [0u8; 32];
    crate::platform_windows::random::fill(&mut key)?;
    let owner = if p.owner_password.is_empty() { p.open_password.as_str() } else { p.owner_password.as_str() };
    let mut perms = Permissions::all();
    if !p.allow_printing {
        perms.remove(Permissions::PRINTABLE | Permissions::PRINTABLE_IN_HIGH_QUALITY);
    }
    if !p.allow_copying {
        perms.remove(Permissions::COPYABLE | Permissions::COPYABLE_FOR_ACCESSIBILITY);
    }
    if !p.allow_editing {
        perms.remove(
            Permissions::MODIFIABLE | Permissions::ANNOTABLE | Permissions::FILLABLE | Permissions::ASSEMBLABLE,
        );
    }
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let state = EncryptionState::try_from(EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: owner,
        user_password: &p.open_password,
        permissions: perms,
    })
    .map_err(|e| format!("could not set up encryption: {e}"))?;
    doc.encrypt(&state).map_err(|e| format!("encryption failed: {e}"))?;
    key.fill(0);
    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| format!("could not write the encrypted document: {e}"))?;
    Ok(out)
}
