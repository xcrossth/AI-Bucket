use std::{fs, path::Path};

use windows_sys::{
    Win32::Foundation::LocalFree,
    Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    },
    Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH},
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
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let temporary = root.join(format!(
        ".provider-{account_id}.credential-{}-{nonce}.tmp",
        std::process::id(),
    ));
    fs::write(&temporary, protected).map_err(|error| error.to_string())?;
    atomic_replace(&temporary, &destination).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    let source_wide = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination_wide = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let moved = unsafe {
        MoveFileExW(
            source_wide.as_ptr(),
            destination_wide.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    if destination.exists() {
        fs::remove_file(destination).map_err(|error| error.to_string())?;
    }
    fs::rename(source, destination).map_err(|error| error.to_string())
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
