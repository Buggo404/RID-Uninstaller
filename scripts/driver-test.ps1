# Installs or removes the RidMonitor driver on a TEST machine (test signing must be on).
# Run elevated.  usage: driver-test.ps1 install <path to rid_monitor.dll>  |  driver-test.ps1 remove
param([Parameter(Mandatory)][ValidateSet('install', 'remove')][string]$Action, [string]$Image)

$ErrorActionPreference = 'Stop'
$name = 'RidMonitor'
$sys = "$env:SystemRoot\System32\drivers\$name.sys"
$service = "HKLM:\SYSTEM\CurrentControlSet\Services\$name"

if ($Action -eq 'remove') {
    fltmc unload $name | Out-Null
    sc.exe delete $name | Out-Null
    Remove-Item $sys -Force -ErrorAction SilentlyContinue
    Get-ChildItem Cert:\LocalMachine\My | Where-Object Subject -eq 'CN=Rid Test Signing' | Remove-Item
    "removed"
    return
}

if (-not (bcdedit /enum '{current}' | Select-String 'testsigning\s+Yes')) {
    throw 'Test signing is off. Run: bcdedit /set testsigning on   then restart.'
}
Copy-Item $Image $sys -Force

# Test signing accepts any certificate; a throwaway self-signed one is enough.
$cert = Get-ChildItem Cert:\LocalMachine\My | Where-Object Subject -eq 'CN=Rid Test Signing' | Select-Object -First 1
if (-not $cert) {
    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject 'CN=Rid Test Signing' -CertStoreLocation Cert:\LocalMachine\My
}
$signed = Set-AuthenticodeSignature -FilePath $sys -Certificate $cert -HashAlgorithm SHA256
"signature: $($signed.Status) $($signed.StatusMessage)"

if (-not (Test-Path $service)) {
    sc.exe create $name type= filesys start= demand error= normal binPath= "System32\drivers\$name.sys" group= "FSFilter Activity Monitor" depend= FltMgr | Out-Null
}
# A minifilter attaches to volumes through an "instance", which carries its altitude.
New-Item "$service\Instances\$name Instance" -Force | Out-Null
Set-ItemProperty "$service\Instances" -Name DefaultInstance -Value "$name Instance"
Set-ItemProperty "$service\Instances\$name Instance" -Name Altitude -Value '385210'
Set-ItemProperty "$service\Instances\$name Instance" -Name Flags -Value 0 -Type DWord

fltmc load $name
fltmc filters | Select-String $name
