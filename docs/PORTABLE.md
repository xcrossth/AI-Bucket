# Portable Build Notes

The portable release is a ZIP containing the release-mode AI Bucket executable, this document,
and the project license. Extract the archive before launching `AI Bucket.exe`; running directly
inside the ZIP is not supported.

## Runtime requirement

AI Bucket uses Tauri and the Microsoft Edge WebView2 Runtime. The runtime is distributed with
supported Windows 10 and Windows 11 installations and is shared by applications, so it is not
duplicated in the portable archive. If WebView2 is missing, install the Evergreen Runtime from
Microsoft or use the AI Bucket installer, which includes Microsoft's bootstrapper.

## Where data is stored

The portable build does not write settings next to the executable. Data remains in the current
Windows profile:

```text
%APPDATA%\com.local.ai-bucket\
```

This includes quota history and DPAPI-encrypted API keys. Moving the executable to another PC or
Windows account does not move or decrypt those credentials.
