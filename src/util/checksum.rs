use std::path::Path;
use std::io::Read;

use sha2::{Sha256, Digest};

use crate::error::{WrightError, Result};

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|e| {
        WrightError::IoError(e)
    })?;
    
    let mut hasher = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer).map_err(|e| {
            WrightError::IoError(e)
        })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn sha256_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_bytes() {
        let hash = sha256_bytes(b"hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }
}
