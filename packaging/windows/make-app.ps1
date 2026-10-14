# Build DisplayCraft and the modified ArgyllCMS on Windows, then a portable
# DisplayCraft folder, zipped, and an installer for it (Inno Setup). Runs on a GitHub Actions Windows runner (see
# .github/workflows/build.yml), or on a Windows machine with rustup.
#
#   packaging\windows\make-app.ps1 [options]
#
#   -Src DIR          this repository (default: the one this script is in)
#   -ArgyllSrc DIR    the modified ArgyllCMS (default: <src>\argyllcms, the
#                     submodule)
#   -Cache DIR        MinGW-w64, jam, Inno Setup, Rust and build output, kept between
#                     builds (default: <src>\tmp\windows-cache)
#   -Out DIR          where the zip and installer go (default: <src>\dist)
#   -Work DIR         scratch space, emptied first
#                     (default: <src>\tmp\windows-work)
#   -RustHome DIR     where rustup put .cargo and .rustup (default: the
#                     user's profile folder; without a rustup there, one is
#                     installed in the cache)
#   -ReleaseTag TAG   a release build for TAG, which must be v<version in
#                     Cargo.toml>
#
# Nothing is taken from the environment: every tool runs in one made here
# from these options and Windows' own folder lookups (Invoke-Tool), so a
# shell's settings can't change the build (jam, for one, makes every
# environment variable a build setting).
param(
    [string]$Src = (Resolve-Path "$PSScriptRoot\..\..").Path,
    [string]$ArgyllSrc = '',
    [string]$Cache = '',
    [string]$Out = '',
    [string]$Work = '',
    [string]$RustHome = [Environment]::GetFolderPath('UserProfile'),
    [string]$ReleaseTag = ''
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

function Full([string]$path) {
    New-Item -ItemType Directory -Force $path | Out-Null
    (Resolve-Path $path).Path
}
$Src = (Resolve-Path $Src).Path
$ArgyllSrc = if ($ArgyllSrc) { (Resolve-Path $ArgyllSrc).Path } else { "$Src\argyllcms" }
$Cache = Full $(if ($Cache) { $Cache } else { "$Src\tmp\windows-cache" })
$Out = Full $(if ($Out) { $Out } else { "$Src\dist" })
$Work = Full $(if ($Work) { $Work } else { "$Src\tmp\windows-work" })

$version = (Select-String -Path "$Src\Cargo.toml" -Pattern '^version = "(.*)"' | Select-Object -First 1).Matches[0].Groups[1].Value
# A release must be of this version, or the release and the zip would
# disagree
if ($ReleaseTag -and $ReleaseTag -ne "v$version") {
    throw "Tag $ReleaseTag doesn't match version $version in Cargo.toml"
}

# The environment every tool gets: Windows' own folders, the cache's temp
# folder, and what's added for each tool
$windows = [Environment]::GetFolderPath('Windows')
New-Item -ItemType Directory -Force "$Cache\tmp" | Out-Null
$baseEnv = @{
    SystemRoot = $windows; windir = $windows; SystemDrive = $windows.Substring(0, 2)
    ComSpec = "$windows\System32\cmd.exe"; PATHEXT = '.COM;.EXE;.BAT;.CMD'
    USERPROFILE = [Environment]::GetFolderPath('UserProfile')
    APPDATA = [Environment]::GetFolderPath('ApplicationData')
    LOCALAPPDATA = [Environment]::GetFolderPath('LocalApplicationData')
    TEMP = "$Cache\tmp"; TMP = "$Cache\tmp"
    Path = "$windows\System32;$windows"
}

# A command line argument, quoted as Windows' C runtimes parse it
function Quote([string]$a) {
    if ($a -notmatch '[\s"]' -and $a -ne '') { return $a }
    '"' + (($a -replace '(\\*)"', '$1$1\"') -replace '(\\+)$', '$1$1') + '"'
}

# Run a program in the base environment plus $Env, in $Dir, appending its
# output to $Log. Returns its exit code.
function Invoke-Tool([string]$Exe, [string[]]$Arguments, [string]$Dir, [hashtable]$Env = @{}, [string]$Log) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $Exe
    $psi.Arguments = ($Arguments | ForEach-Object { Quote $_ }) -join ' '
    $psi.WorkingDirectory = $Dir
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.EnvironmentVariables.Clear()
    foreach ($e in @($baseEnv, $Env)) { foreach ($k in $e.Keys) { $psi.EnvironmentVariables[$k] = $e[$k] } }
    $p = [System.Diagnostics.Process]::Start($psi)
    # Both streams at once, or a full pipe would stall the program
    $o = $p.StandardOutput.ReadToEndAsync()
    $r = $p.StandardError.ReadToEndAsync()
    $p.WaitForExit()
    Add-Content -Path $Log -Value ($o.Result + $r.Result)
    $p.ExitCode
}

# A download checked against the checksum this was written for
function Get-Checked([string]$Url, [string]$File, [string]$Sha256) {
    if (-not (Test-Path $File) -or (Get-FileHash -Algorithm SHA256 $File).Hash -ne $Sha256) {
        Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $File
        if ((Get-FileHash -Algorithm SHA256 $File).Hash -ne $Sha256) { throw "$File doesn't match its checksum" }
    }
}

$log = "$Work\build.log"
Remove-Item -Recurse -Force "$Work\*" -ErrorAction SilentlyContinue
Set-Content -Path $log -Value ''

# MinGW-w64: WinLibs' GCC with the UCRT runtime, self-contained in one folder
$mingw = "$Cache\mingw64"
if (-not (Test-Path "$mingw\bin\gcc.exe")) {
    Get-Checked 'https://github.com/brechtsanders/winlibs_mingw/releases/download/16.2.0posix-14.0.0-ucrt-r2/winlibs-x86_64-posix-seh-gcc-16.2.0-mingw-w64ucrt-14.0.0-r2.zip' `
        "$Cache\mingw.zip" 'D5DBAFC4A170E762CA6143151EC918FB9E2C72736FB14CD704ABEBC6BDD5276A'
    tar -xf "$Cache\mingw.zip" -C $Cache
}
$mingwPath = "$mingw\bin;$windows\System32;$windows"

# jam: ArgyllCMS's (Jam 2.6.1 with its patches). Built with NT defined, as
# Argyll's Jambase recognises Windows only as OS=NT (-DMINGW64 would make it
# OS=MINGW64); -fpermissive for old C current GCC rejects. Only jam0: the
# second stage just rebuilds it with itself, and Argyll names its Jambase
# with -f anyway.
$jam = "$Cache\ajam\jam0.exe"
if (-not (Test-Path $jam)) {
    Get-Checked 'https://www.argyllcms.com/ajam-2.5.2-1.3.4.zip' "$Cache\ajam.zip" '1A447347A4CB1090F0715ED96714CEE0F5D3EAB730DA9374EC1F394FFA5D608D'
    New-Item -ItemType Directory -Force "$Cache\ajam" | Out-Null
    tar -xf "$Cache\ajam.zip" -C "$Cache\ajam"
    $sources = 'builtins.c command.c compile.c execunix.c execvms.c expand.c filent.c fileos2.c fileunix.c filevms.c glob.c hash.c headers.c jam.c jambase.c jamgram.c lists.c make.c make1.c newstr.c option.c parse.c pathunix.c pathvms.c regexp.c rules.c scan.c search.c timestamp.c variable.c' -split ' '
    $rc = Invoke-Tool "$mingw\bin\gcc.exe" (@('-O2', '-fpermissive', '-w', '-DNT', '-o', 'jam0.exe') + $sources) "$Cache\ajam" @{ Path = $mingwPath } $log
    if ($rc -ne 0) { Get-Content $log -Tail 30; throw "Building jam failed" }
}

# Rust's Windows GNU toolchain, linked with the MinGW-w64 above: an installed
# rustup (as on GitHub's runners), or one kept in the cache
$cargoHome = "$RustHome\.cargo"; $rustupHome = "$RustHome\.rustup"
if (-not (Test-Path "$cargoHome\bin\rustup.exe")) {
    $cargoHome = "$Cache\cargo"; $rustupHome = "$Cache\rustup"
    if (-not (Test-Path "$cargoHome\bin\rustup.exe")) {
        Invoke-WebRequest -UseBasicParsing -Uri 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-gnu/rustup-init.exe' -OutFile "$Cache\rustup-init.exe"
        $rc = Invoke-Tool "$Cache\rustup-init.exe" @('-y', '--default-host', 'x86_64-pc-windows-gnu', '--default-toolchain', 'none', '--profile', 'minimal', '--no-modify-path') $Cache @{ CARGO_HOME = $cargoHome; RUSTUP_HOME = $rustupHome } $log
        if ($rc -ne 0) { Get-Content $log -Tail 30; throw "Installing rustup failed" }
    }
}
$rustEnv = @{ CARGO_HOME = $cargoHome; RUSTUP_HOME = $rustupHome; Path = "$cargoHome\bin;$mingwPath" }
$rc = Invoke-Tool "$cargoHome\bin\rustup.exe" @('toolchain', 'install', 'stable-x86_64-pc-windows-gnu', '--profile', 'minimal') $Cache $rustEnv $log
if ($rc -ne 0) { Get-Content $log -Tail 30; throw "Installing Rust failed" }

# Sources, copied so the trees aren't written to, leaving out the folders
# named in $Skip (by name, at any depth)
function Copy-Tree([string]$From, [string]$To, [string[]]$Skip) {
    $rc = Invoke-Tool "$windows\System32\robocopy.exe" (@($From, $To, '/MIR', '/NFL', '/NDL', '/NJH', '/NJS', '/NP', '/XD') + $Skip) $Src @{} $log
    # robocopy's codes below 8 are successes
    if ($rc -ge 8) { Get-Content $log -Tail 30; throw "Copying $From failed" }
}
# Not ArgyllCMS's "target" folder, which is source (targen)
Copy-Tree $ArgyllSrc "$Work\argyllcms" @('.git')
# DisplayCraft's build output and scratch folders; the submodule is copied
# on its own
Copy-Tree $Src "$Work\displaycraft" @('.git', 'target', 'tmp', 'dist', 'argyllcms')

# ArgyllCMS, with its own copies of TIFF, JPEG, PNG, zlib and SSL. DESTDIR
# is relative, as jam can't create a path starting with a drive letter.
$rc = Invoke-Tool $jam @('-q', '-fJambase', "-j$([Environment]::ProcessorCount)", '-sBUILTIN_TIFF=true', '-sBUILTIN_JPEG=true', '-sBUILTIN_PNG=true', '-sBUILTIN_Z=true', '-sBUILTIN_SSL=true', '-sDESTDIR=..\argyll', 'install') `
    "$Work\argyllcms" @{ Path = $mingwPath; MINGW = $mingw } $log
if ($rc -ne 0 -or -not (Test-Path "$Work\argyll\bin\dispcal.exe")) { Get-Content $log -Tail 60; throw "Building ArgyllCMS failed" }

# DisplayCraft
$rc = Invoke-Tool "$cargoHome\bin\cargo.exe" @('+stable-x86_64-pc-windows-gnu', 'build', '--release', '--locked') "$Work\displaycraft" `
    ($rustEnv + @{ CARGO_TARGET_DIR = "$Cache\target" }) $log
if ($rc -ne 0) { Get-Content $log -Tail 60; throw "Building DisplayCraft failed" }

# The folder: displaycraft.exe, with its ArgyllCMS in argyll\bin
# (system::Single::bundled_dir) and the corrections beside it in argyll\ref
$app = "$Work\DisplayCraft"
New-Item -ItemType Directory -Force "$app\argyll\bin", "$app\doc" | Out-Null
Copy-Item "$Cache\target\release\displaycraft.exe", "$Src\assets\displaycraft.ico" $app
foreach ($p in 'dispcal', 'dispread', 'dispwin', 'targen', 'colprof', 'fakeread', 'colverify', 'spotread', 'ccxxmake', 'oeminst', 'iccdump') {
    Copy-Item "$Work\argyll\bin\$p.exe" "$app\argyll\bin\"
}
Copy-Item -Recurse "$Work\argyll\ref" "$app\argyll\ref"
Copy-Item "$Src\LICENSE-MIT", "$Src\LICENSE-APACHE", "$Src\NOTICE" "$app\doc\"
Copy-Item "$ArgyllSrc\License.txt" "$app\doc\ArgyllCMS-LICENSE.txt"
# Which source this was built from, as the AGPL asks
$commit = 'unknown'
$git = (Get-Command git -ErrorAction SilentlyContinue).Source
if ($git) {
    $gitEnv = @{ Path = "$(Split-Path $git);$windows\System32;$windows"; GIT_CONFIG_GLOBAL = "$Cache\gitconfig" }
    $commitLog = "$Work\commit.txt"
    if ((Invoke-Tool $git @('-C', $ArgyllSrc, 'rev-parse', 'HEAD') $Src $gitEnv $commitLog) -eq 0) {
        $commit = (Get-Content $commitLog | Where-Object { $_ -match '^[0-9a-f]{40}$' } | Select-Object -First 1)
    }
}
(Get-Content "$Src\packaging\ARGYLLCMS.txt") + "    Commit: $commit" | Set-Content "$app\doc\ARGYLLCMS.txt"

# Only DLLs Windows itself has, or it won't run elsewhere (Argyll links
# MinGW's runtime statically, and Rust's GNU target its own)
$missing = @()
foreach ($f in Get-ChildItem "$app\displaycraft.exe", "$app\argyll\bin\*.exe") {
    $dumpLog = "$Work\imports.txt"
    Set-Content -Path $dumpLog -Value ''
    Invoke-Tool "$mingw\bin\objdump.exe" @('-p', $f.FullName) $Work @{ Path = $mingwPath } $dumpLog | Out-Null
    foreach ($m in (Select-String -Path $dumpLog -Pattern 'DLL Name: (.*)$')) {
        $dll = $m.Matches[0].Groups[1].Value.Trim()
        if (-not (Test-Path "$windows\System32\$dll") -and $dll -notlike 'api-ms-win-*') { $missing += "$($f.Name): $dll" }
    }
}
if ($missing) { $missing | Sort-Object -Unique; throw "Needs DLLs that aren't part of Windows (above)" }

$zip = "$Out\DisplayCraft-$version-windows-x64.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path $app -DestinationPath $zip

# The installer. Inno Setup is installed in the cache, for the current user
# only, so no administrator rights are needed.
$iscc = "$Cache\innosetup\ISCC.exe"
if (-not (Test-Path $iscc)) {
    Get-Checked 'https://github.com/jrsoftware/issrc/releases/download/is-7_1_0/innosetup-7.1.0-x64.exe' `
        "$Cache\innosetup.exe" '0362A383ED217D4C4239B5933866DD96D3EB2102737DA92F80F6057A4B40DF2F'
    $rc = Invoke-Tool "$Cache\innosetup.exe" @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/CURRENTUSER', '/NOICONS', "/DIR=$Cache\innosetup") $Cache @{} $log
    if ($rc -ne 0 -or -not (Test-Path $iscc)) { Get-Content $log -Tail 30; throw "Installing Inno Setup failed" }
}
$rc = Invoke-Tool $iscc @('/Q', "/DAppVersion=$version", "/DAppDir=$app", "/DOutDir=$Out", "$Src\packaging\windows\DisplayCraft.iss") $Work @{} $log
if ($rc -ne 0) { Get-Content $log -Tail 40; throw "Building the installer failed" }

Get-Item $zip, "$Out\DisplayCraft-$version-windows-x64-setup.exe" | Select-Object FullName, Length | Format-List
