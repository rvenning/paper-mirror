//! Windows Data Protection API: encrypts data for the current Windows user, so stored
//! signatures are unreadable to other accounts and when copied to another machine.

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};

/// Extra entropy binds the blobs to Paper Mirror.
const ENTROPY: &[u8] = b"Paper Mirror signatures v1";

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    let v = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe { LocalFree(out.pbData.cast()) };
    v
}

pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    let input = blob(plain);
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            &entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 { Err("Windows could not protect the data.".into()) } else { Ok(take(out)) }
}

pub fn unprotect(cipher: &[u8]) -> Result<Vec<u8>, String> {
    let input = blob(cipher);
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            &entropy,
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
    };
    if ok == 0 { Err("The protected data could not be read by this Windows account.".into()) } else { Ok(take(out)) }
}

#[cfg(test)]
mod tests {
    #[test]
    fn round_trip_and_ciphertext_differs() {
        let secret = b"signature strokes";
        let c = super::protect(secret).unwrap();
        assert!(!c.windows(secret.len()).any(|w| w == secret));
        assert_eq!(super::unprotect(&c).unwrap(), secret);
        assert!(super::unprotect(b"garbage").is_err());
    }
}
