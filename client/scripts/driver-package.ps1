# Packs the driver into the .cab that the Microsoft Hardware Dev Center takes for
# attestation signing. Run after building the driver (scripts\build.ps1).
#
#   scripts\driver-package.ps1 -Thumbprint <thumbprint of the EV certificate>
#
# What happens next is done by hand in Partner Center (https://partner.microsoft.com/dashboard/hardware):
#   1. New hardware submission -> upload target\driver\RidMonitor.cab
#   2. Leave "Perform test-signing" off, tick the Windows 10/11 x64 signatures, submit.
#   3. Download the signed package; its RidMonitor.sys is the one to ship.
# The account must have this EV certificate registered. The .cab itself must be signed
# with it, which this script does; the driver file inside is signed by Microsoft.
#
# NOT YET RUN END TO END: there is no Partner Center account to submit to, and inf2cat was
# not available on the build machine, so neither the catalog step nor the submission has
# been exercised.
param([Parameter(Mandatory)][string]$Thumbprint)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$out = "$root\target\driver"
$stage = "$out\RidMonitor"

New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item "$root\driver\rid-monitor\target\release\rid_monitor.dll" "$stage\RidMonitor.sys" -Force
Copy-Item "$root\driver\rid-monitor\target\release\rid_monitor.pdb" "$stage\RidMonitor.pdb" -Force
Copy-Item "$root\driver\rid-monitor\RidMonitor.inf" $stage -Force

$ddf = @"
.OPTION EXPLICIT
.Set CabinetFileCountThreshold=0
.Set FolderFileCountThreshold=0
.Set FolderSizeThreshold=0
.Set MaxCabinetSize=0
.Set MaxDiskFileCount=0
.Set MaxDiskSize=0
.Set CompressionType=MSZIP
.Set Cabinet=on
.Set Compress=on
.Set CabinetNameTemplate=RidMonitor.cab
.Set DiskDirectoryTemplate=$out
.Set DestinationDir=RidMonitor
$stage\RidMonitor.inf
$stage\RidMonitor.sys
$stage\RidMonitor.pdb
"@
Set-Content "$out\RidMonitor.ddf" $ddf -Encoding ASCII
makecab /f "$out\RidMonitor.ddf" | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'makecab failed' }

$signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" | Sort-Object FullName | Select-Object -Last 1
& $signtool.FullName sign /sha1 $Thumbprint /fd SHA256 /tr http://timestamp.globalsign.com/tsa/r6advanced1 /td SHA256 "$out\RidMonitor.cab"
if ($LASTEXITCODE -ne 0) { throw 'signing the cabinet failed' }
"ready to submit: $out\RidMonitor.cab"
