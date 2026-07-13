param(
    [Parameter(Mandatory = $true)]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory,

    [string]$BinaryPath = "src-tauri/target/release/ai-bucket.exe"
)

$ErrorActionPreference = "Stop"
$binary = Resolve-Path -LiteralPath $BinaryPath
$license = Resolve-Path -LiteralPath "LICENSE"
$portableNotes = Resolve-Path -LiteralPath "docs/PORTABLE.md"
$output = [IO.Path]::GetFullPath($OutputDirectory)
$name = "AI-Bucket_${Version}_windows-x64-portable"
$stage = [IO.Path]::GetFullPath((Join-Path $output $name))
$zip = [IO.Path]::GetFullPath((Join-Path $output "$name.zip"))
$outputPrefix = $output.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (!$stage.StartsWith($outputPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Portable staging path escaped the requested output directory."
}

New-Item -ItemType Directory -Path $output -Force | Out-Null
if (Test-Path -LiteralPath $stage) {
    Remove-Item -LiteralPath $stage -Recurse -Force
}
if (Test-Path -LiteralPath $zip) {
    Remove-Item -LiteralPath $zip -Force
}

New-Item -ItemType Directory -Path $stage | Out-Null
Copy-Item -LiteralPath $binary -Destination (Join-Path $stage "AI Bucket.exe")
Copy-Item -LiteralPath $portableNotes -Destination (Join-Path $stage "README.md")
Copy-Item -LiteralPath $license -Destination (Join-Path $stage "LICENSE")
Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -CompressionLevel Optimal
Remove-Item -LiteralPath $stage -Recurse -Force

Write-Output $zip
