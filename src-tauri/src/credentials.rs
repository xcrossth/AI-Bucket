use std::{fs, path::Path};

use windows_sys::{
    Win32::Foundation::LocalFree,
    Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    },
};

fn transform(data: &[u8], protect: bool) -> Result<Vec<u8>, String> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let success = unsafe {
        if protect {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if success == 0 || output.pbData.is_null() {
        return Err("Windows could not protect the provider credential.".into());
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    let _ = unsafe { LocalFree(output.pbData.cast()) };
    Ok(result)
}

fn path(root: &Path, account_id: i64) -> std::path::PathBuf {
    root.join(format!("provider-{account_id}.credential"))
}

pub fn write(root: &Path, account_id: i64, value: &str) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let destination = path(root, account_id);
    if value.is_empty() {
        if destination.exists() {
            fs::remove_file(destination).map_err(|error| error.to_string())?;
        }
        return Ok(());
    }
    let protected = transform(value.as_bytes(), true)?;
    fs::write(destination, protected).map_err(|error| error.to_string())
}

pub fn read(root: &Path, account_id: i64) -> Result<String, String> {
    let source = path(root, account_id);
    if !source.is_file() {
        return Ok(String::new());
    }
    let protected = fs::read(source).map_err(|error| error.to_string())?;
    let plain = transform(&protected, false)?;
    String::from_utf8(plain).map_err(|_| "Stored provider credential is invalid.".into())
}

pub fn mask(value: &str) -> String {
    if value.is_empty() {
        return String::new();
    }
    let prefix: String = value.chars().take(7).collect();
    format!("{prefix}************")
}

pub fn is_masked(value: &str) -> bool {
    value.contains('*')
}
