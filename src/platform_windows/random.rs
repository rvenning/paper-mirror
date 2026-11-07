//! Cryptographically secure random bytes from the Windows CNG system RNG.

use windows_sys::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};

pub fn fill(buf: &mut [u8]) -> Result<(), String> {
    let status = unsafe {
        BCryptGenRandom(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
    };
    if status == 0 { Ok(()) } else { Err(format!("BCryptGenRandom failed (0x{status:08x})")) }
}

/// A random u64 (for unique temporary names).
pub fn u64() -> Result<u64, String> {
    let mut b = [0u8; 8];
    fill(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

#[cfg(test)]
mod tests {
    #[test]
    fn produces_distinct_output() {
        let (mut a, mut b) = ([0u8; 32], [0u8; 32]);
        super::fill(&mut a).unwrap();
        super::fill(&mut b).unwrap();
        assert_ne!(a, b);
        assert_ne!(a, [0u8; 32]);
    }
}
