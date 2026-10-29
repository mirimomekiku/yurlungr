# yurlungr installer for Windows (PowerShell 5.1 or newer).
#
#   irm https://raw.githubusercontent.com/mirimomekiku/yurlungr/main/install.ps1 | iex
#
# From Command Prompt (cmd.exe):
#   powershell -NoProfile -ExecutionPolicy Bypass -Command "irm https://raw.githubusercontent.com/mirimomekiku/yurlungr/main/install.ps1 | iex"
#
# Options are environment variables, because `irm | iex` cannot take parameters:
#   $env:YURLUNGR_VERSION         install a specific release tag (default: latest)
#   $env:YURLUNGR_INSTALL_DIR     install here (default: %LOCALAPPDATA%\Programs\yurlungr)
#   $env:YURLUNGR_FROM_SOURCE=1   build with cargo instead of downloading a binary
#   $env:YURLUNGR_NO_MODIFY_PATH=1  leave the user PATH alone
#   $env:YURLUNGR_UNINSTALL=1     remove yurlungr, its agent hooks and its PATH entry
#   $env:YURLUNGR_REPO, $env:YURLUNGR_RELEASE_URL, $env:YURLUNGR_GIT_URL, $env:NO_COLOR
#
# Everything lives in a function so a download that is cut short cannot run half a script.

function Install-Yurlungr {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'   # the built-in progress bar makes downloads very slow in 5.1

    $repo = if ($env:YURLUNGR_REPO) { $env:YURLUNGR_REPO } else { 'mirimomekiku/yurlungr' }
    $version = if ($env:YURLUNGR_VERSION) { $env:YURLUNGR_VERSION } else { 'latest' }
    $gitUrl = if ($env:YURLUNGR_GIT_URL) { $env:YURLUNGR_GIT_URL } else { "https://github.com/$repo" }
    $installDir = if ($env:YURLUNGR_INSTALL_DIR) { $env:YURLUNGR_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\yurlungr' }
    $fromSource = [bool]$env:YURLUNGR_FROM_SOURCE
    $modifyPath = -not [bool]$env:YURLUNGR_NO_MODIFY_PATH
    $uninstall = [bool]$env:YURLUNGR_UNINSTALL

    # ---- terminal ------------------------------------------------------------
    try { [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 } catch { }
    try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch { }

    $redirected = $false
    try { $redirected = [Console]::IsOutputRedirected } catch { }
    $esc = [char]27
    # Escape sequences need Windows Terminal, PowerShell 7, or another modern host.
    $vt = (-not $env:NO_COLOR) -and (-not $redirected) -and (
        ($PSVersionTable.PSVersion.Major -ge 7) -or $env:WT_SESSION -or $env:TERM_PROGRAM -or ($env:ConEmuANSI -eq 'ON'))
    $trueColor = $vt -and ($env:WT_SESSION -or ($env:COLORTERM -match 'truecolor|24bit') -or ($PSVersionTable.PSVersion.Major -ge 7))
    $plain = [bool]$env:NO_COLOR -or $redirected

    # ---- the rainbow ---------------------------------------------------------
    # $phase turns the colours round the hue circle: 0 is the plain rainbow, 1 the
    # same picture again. Show-Banner raises it to make the colours flow.
    $phase = 0.0

    # position 0..1 -> r, g, b (the same rainbow as the Linux installer and the app);
    # $k dims it (1 = the lit face of the snake, 0.6 its side, 0.3 its shadow)
    function Get-RainbowRgb([double]$position, [double]$k = 1.0) {
        $turn = [Math]::Min(1.0, [Math]::Max(0.0, $position)) * 0.83 - $phase
        $hue = ($turn - [Math]::Floor($turn)) * 6
        $sector = [int][Math]::Floor($hue)
        $f = $hue - [Math]::Floor($hue)
        $low = 0.1
        $t = 1 - 0.9 * (1 - $f)
        $q = 1 - 0.9 * $f
        switch ($sector) {
            0 { $r = 1.0; $g = $t; $b = $low }
            1 { $r = $q; $g = 1.0; $b = $low }
            2 { $r = $low; $g = 1.0; $b = $t }
            3 { $r = $low; $g = $q; $b = 1.0 }
            4 { $r = $t; $g = $low; $b = 1.0 }
            default { $r = 1.0; $g = $low; $b = $q }
        }
        $up = [MidpointRounding]::AwayFromZero
        $rgb = @([Math]::Round($r * 255, $up), [Math]::Round($g * 255, $up), [Math]::Round($b * 255, $up))
        return @($rgb | ForEach-Object { [int][Math]::Round($_ * $k, $up) })
    }

    function Get-Ansi256([int[]]$rgb) {
        $lvl = { param($c) [int][Math]::Round($c / 255 * 5) }
        return 16 + 36 * (& $lvl $rgb[0]) + 6 * (& $lvl $rgb[1]) + (& $lvl $rgb[2])
    }

    # The nearest of the 16 classic console colours, for hosts without escape sequences.
    function Get-ConsoleColour([double]$position) {
        $order = @('Red', 'Yellow', 'Green', 'Cyan', 'Blue', 'Magenta')
        $hue = [Math]::Min(1.0, [Math]::Max(0.0, $position)) * 0.83 * 6
        return $order[[Math]::Min(5, [int][Math]::Floor($hue + 0.5))]
    }

    # Write text with the rainbow running across `$width` columns.
    function Write-Rainbow([string]$text, [int]$width, [string]$trailRow = '', [switch]$NoNewline) {
        $last = [Math]::Max(1, $width - 1)
        # along the snake's body when a trail row is given, else across the text
        function Get-TrailPosition([int]$i) {
            if ($trailRow) { return $trailDigits.IndexOf($trailRow[$i]) / 61.0 }
            return $i / $last
        }
        if ($plain) {
            if ($NoNewline) { Write-Host $text -NoNewline } else { Write-Host $text }
            return
        }
        if ($vt) {
            $sb = New-Object System.Text.StringBuilder
            for ($i = 0; $i -lt $text.Length; $i++) {
                $ch = $text[$i]
                if ($ch -ne ' ') {
                    $k = 1.0
                    if ($ch -eq [char]0x2593) { $k = 0.6 } elseif ($ch -eq [char]0x2591) { $k = 0.3 }
                    $rgb = Get-RainbowRgb (Get-TrailPosition $i) $k
                    if ($trueColor) { [void]$sb.Append("$esc[38;2;$($rgb[0]);$($rgb[1]);$($rgb[2])m") }
                    else { [void]$sb.Append("$esc[38;5;$(Get-Ansi256 $rgb)m") }
                }
                [void]$sb.Append($ch)
            }
            [void]$sb.Append("$esc[0m")
            if ($NoNewline) { Write-Host $sb.ToString() -NoNewline } else { Write-Host $sb.ToString() }
            return
        }
        for ($i = 0; $i -lt $text.Length; $i++) {
            $ch = [string]$text[$i]
            if ($ch -eq ' ') { Write-Host ' ' -NoNewline }
            else { Write-Host $ch -NoNewline -ForegroundColor (Get-ConsoleColour (Get-TrailPosition $i)) }
        }
        if (-not $NoNewline) { Write-Host '' }
    }

    # The snake, ASCII-coded (# full block, ^ upper half, v lower half; % side, : shadow).
    # It is the same picture as assets/snake.txt, made by scripts/make-snake.py.
    $art = @(
        '       vvvvvvvvvvvvvvvvvvvvvvvv  vv######v',
        '    v#################################%%###v      v',
        '   ############################^#############vvvv^%%%',
        '  #####%%%%%%%%%%%%%%%%%%%%%%%%%%^^#####^^^^%%%%%^v%::',
        '  #####%%::::::::::::::::::::::::: %%%%%%%%%%%:::::%%',
        '   ################################vv :::::          :',
        '    ^#################################v',
        '      %^^^^^^^^^^^^^^^^^^^^^^^^^^^^####v%',
        '        :%%%%%%%%%%%%%%%%%%%%%%%%%%#####%%',
        '                   vvvvvvvvvvvvvvvv####^%%:',
        'vvvvvvv###############################^%%%:',
        ' %%%%%%^^^^^^^^^^^^################^^%%%%:',
        '   ::::::%%%%%%%%%%%%%%%%%%%%%%%%%%%%%%::',
        '                      ::::::::::::::::'
    )
    # Where each cell lies along the snake's body (0 = tail tip .. z/Z = head), one
    # base-62 digit per cell. The rainbow is laid along this trail.
    $trail = @(
        '       EEEEEFFGGHHIIJJKLLMMNNOO  QQRSSTTUU',
        '    DDDDDDEEFFGGHHIIJJKLLMMNNOOPPQQRSSSSUUVV      Z',
        '   CCCCCDDEEFFGGHHIIJJKLLMMNNOOPPQQRSSTTUUVVWWXXYYYZZ',
        '  CBBBBCCCDDEEFFGHHIIJJKKLLMMNNOOQQRSSTTUUVVVWWXXYZYYZ',
        '  CBBAABBCCDDEEFFGGHHIIJJKLLMMNNOO QRRSSTTUUVVVWWXXZZ',
        '   BBAAzzyyxxwwvuuttssrrqqppoonnnnnmm RSSTT          Z',
        '    BAAzzyyxxwwvuuttssrrqqppoonmmmmmmml',
        '      Azzyyxxwwvuuttssrrqqppoonmmllllllll',
        '        Azzyyxxwwvuuttssrrqqppoonmmkkkklll',
        '                   abbccddeefgghhiijjkkllll',
        '011223344556677899aabbccddeefgghhiijjkkkkll',
        ' 00112244556677899aabbccddeefgghhiijjjkkkk',
        '   01122344556677899aabccddeeffgghhiijjjk',
        '                      abbccddeefgghhii'
    )
    $trailDigits = '0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ'

    # One picture of the banner, in the current $phase.
    function Write-Banner {
        Write-Host ''
        for ($row = 0; $row -lt $art.Count; $row++) {
            $line = $art[$row].Replace('#', [string][char]0x2588).Replace('^', [string][char]0x2580).Replace('v', [string][char]0x2584).Replace('%', [string][char]0x2593).Replace(':', [string][char]0x2591)
            Write-Rainbow $line 54 $trail[$row]
        }
        Write-Host ''
        Write-Host '  ' -NoNewline
        Write-Rainbow 'y u r l u n g r' 15
        if ($vt) { Write-Host "  $esc[2ma terminal workspace for coding agents$esc[0m" }
        else { Write-Host '  a terminal workspace for coding agents' }
        Write-Host ''
    }

    # The rainbow flows once round the snake, then it settles. Only where escape
    # sequences work and the window is tall enough, and not with $env:YURLUNGR_NO_ANIM=1.
    function Show-Banner {
        $height = 30
        try { $height = [Console]::WindowHeight } catch { }
        $noAnim = $env:YURLUNGR_NO_ANIM -and $env:YURLUNGR_NO_ANIM -ne '0'
        if ($vt -and -not $plain -and -not $noAnim -and $height -ge 22) {
            $lines = $art.Count + 5
            Write-Host "$esc[?25l" -NoNewline
            try {
                for ($frame = 0; $frame -lt 24; $frame++) {
                    $phase = $frame / 24
                    Write-Banner
                    Write-Host "$esc[${lines}A" -NoNewline
                    Start-Sleep -Milliseconds 55
                }
            } finally {
                Write-Host "$esc[?25h" -NoNewline
            }
        }
        $phase = 0.0
        Write-Banner
    }

    # ---- messages ------------------------------------------------------------
    $tick = [string][char]0x2714
    $cross = [string][char]0x2718
    $dot = [string][char]0x25C6
    # Wipe the spinner line (not when output goes to a file or pipe).
    function Clear-Line { if (-not $redirected) { Write-Host "`r$(' ' * 78)`r" -NoNewline } }
    function Write-Ok([string]$msg) { Clear-Line; Write-Host '  ' -NoNewline; Write-Host $tick -NoNewline -ForegroundColor Green; Write-Host " $msg" }
    function Write-Bad([string]$msg) { Clear-Line; Write-Host '  ' -NoNewline; Write-Host $cross -NoNewline -ForegroundColor Red; Write-Host " $msg" }
    function Write-Note([string]$msg) { Write-Host '  ' -NoNewline; Write-Host $dot -NoNewline -ForegroundColor Yellow; Write-Host " $msg" }
    function Stop-Install([string]$msg, [string]$hint) {
        Write-Bad $msg
        if ($hint) { Write-Host "    $hint" }
        throw 'yurlungr-install-aborted'
    }

    $frames = @('-', '\', '|', '/')
    if (-not $plain) { $frames = @(0x280B, 0x2819, 0x2839, 0x2838, 0x283C, 0x2834, 0x2826, 0x2827, 0x2807, 0x280F | ForEach-Object { [string][char]$_ }) }
    $spinColours = @('Red', 'Yellow', 'Green', 'Cyan', 'Blue', 'Magenta')
    function Show-Spinner([int]$i, [string]$msg) {
        if ($redirected) { return }
        Write-Host "`r  " -NoNewline
        Write-Host $frames[$i % $frames.Count] -NoNewline -ForegroundColor $spinColours[$i % $spinColours.Count]
        Write-Host " $msg" -NoNewline
    }

    # Download a file behind a spinner. Returns $true, or $false for a 404 (no such release).
    function Get-RemoteFile([string]$url, [string]$dest, [string]$msg, [switch]$Soft) {
        $client = New-Object System.Net.WebClient
        $client.Headers['User-Agent'] = 'yurlungr-installer'
        try {
            $task = $client.DownloadFileTaskAsync($url, $dest)
            $i = 0
            while (-not $task.IsCompleted) {
                Show-Spinner $i $msg
                $i++
                Start-Sleep -Milliseconds 100
            }
            if ($task.IsFaulted) { throw $task.Exception.InnerException }
            Write-Ok $msg
            return $true
        } catch {
            $status = $null
            $ex = $_.Exception
            while ($ex -and -not $status) {
                if ($ex -is [System.Net.WebException] -and $ex.Response) { $status = [int]$ex.Response.StatusCode }
                $ex = $ex.InnerException
            }
            if ($Soft -and $status -eq 404) { Clear-Line; return $false }
            Stop-Install "$msg failed" "$($_.Exception.Message)"
        } finally {
            $client.Dispose()
        }
    }

    function Add-ToUserPath([string]$dir) {
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        try {
            # Read the raw value so %VARIABLES% in the existing PATH survive.
            $current = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $parts = @($current -split ';' | Where-Object { $_ })
            $already = $parts | Where-Object { $_.TrimEnd('\') -ieq $dir.TrimEnd('\') }
            if (-not $already) {
                $new = [string]::Join(';', @($parts + $dir))
                $key.SetValue('Path', $new, [Microsoft.Win32.RegistryValueKind]::ExpandString)
                return $true
            }
            return $false
        } finally {
            $key.Close()
        }
    }

    function Remove-FromUserPath([string]$dir) {
        $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
        try {
            $current = [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
            $parts = @($current -split ';' | Where-Object { $_ })
            $kept = @($parts | Where-Object { $_.TrimEnd('\') -ine $dir.TrimEnd('\') })
            if ($kept.Count -ne $parts.Count) {
                $key.SetValue('Path', [string]::Join(';', $kept), [Microsoft.Win32.RegistryValueKind]::ExpandString)
                return $true
            }
            return $false
        } finally {
            $key.Close()
        }
    }

    # Tell running programs (Explorer, new terminals) that the environment changed.
    function Send-EnvironmentChange {
        try {
            Add-Type -Namespace Yurlungr -Name Native -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Auto)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
'@
            $result = [UIntPtr]::Zero
            [void][Yurlungr.Native]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
        } catch { }
    }

    $exe = Join-Path $installDir 'yurlungr.exe'
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("yurlungr-install-" + [System.Guid]::NewGuid().ToString('N'))

    try {
        if (($env:OS -ne 'Windows_NT') -and (-not $IsWindows)) {
            Show-Banner
            Stop-Install 'This installer is for Windows.' 'On Linux or macOS run:  curl -fsSL https://raw.githubusercontent.com/mirimomekiku/yurlungr/main/install.sh | sh'
        }
        New-Item -ItemType Directory -Path $tmp -Force | Out-Null
        Show-Banner

        # ---- uninstall -------------------------------------------------------
        if ($uninstall) {
            if (Test-Path $exe) {
                $hooksRemoved = $false
                try {
                    & $exe integration uninstall 2>&1 | Out-Null
                    $hooksRemoved = ($LASTEXITCODE -eq 0)
                } catch { }
                if ($hooksRemoved) { Write-Ok 'Removed agent hooks' }
                else { Write-Note 'Could not remove the agent hooks; run: yurlungr integration uninstall' }
                try {
                    Remove-Item -Path $exe -Force
                    Write-Ok "Removed $exe"
                } catch {
                    # A running session keeps the file locked; rename it so it can go later.
                    Move-Item -Path $exe -Destination "$exe.old" -Force
                    Write-Note 'yurlungr is still running; it was moved aside and will be gone once it exits.'
                }
            } else {
                Write-Note "No yurlungr found in $installDir."
            }
            if (Remove-FromUserPath $installDir) { Send-EnvironmentChange; Write-Ok 'Removed the PATH entry' }
            if ((Test-Path $installDir) -and -not (Get-ChildItem $installDir -Force -ErrorAction SilentlyContinue)) { Remove-Item $installDir -Force }
            Write-Host ''
            Write-Host '  Your settings and sessions were kept (%APPDATA%\yurlungr and %LOCALAPPDATA%\yurlungr).'
            Write-Host ''
            return
        }

        # ---- platform ---------------------------------------------------------
        $arch = $env:PROCESSOR_ARCHITECTURE
        if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
        if ($arch -eq 'ARM64') {
            Write-Note 'ARM64 Windows: using the x64 build, which Windows runs under emulation.'
        } elseif ($arch -ne 'AMD64') {
            $fromSource = $true
            Write-Note "No prebuilt binary for $arch; building from source."
        }
        $os = (Get-CimInstance Win32_OperatingSystem -ErrorAction SilentlyContinue).Caption
        Write-Ok "Platform: $(if ($os) { $os } else { 'Windows' }) ($arch)"

        $upgrading = Test-Path $exe
        $placed = $false

        # ---- download a release -------------------------------------------------
        if (-not $fromSource) {
            $asset = 'yurlungr-x86_64-pc-windows-msvc.zip'
            if ($env:YURLUNGR_RELEASE_URL) { $base = $env:YURLUNGR_RELEASE_URL.TrimEnd('/') }
            elseif ($version -eq 'latest') { $base = "https://github.com/$repo/releases/latest/download" }
            else { $base = "https://github.com/$repo/releases/download/$version" }
            $zip = Join-Path $tmp $asset
            $found = Get-RemoteFile "$base/$asset" $zip "Downloading yurlungr (windows x64)" -Soft
            if ($found) {
                [void](Get-RemoteFile "$base/$asset.sha256" "$zip.sha256" 'Fetching checksum')
                $want = ((Get-Content "$zip.sha256" -Raw) -split '\s+')[0].Trim().ToLower()
                $got = (Get-FileHash -Algorithm SHA256 -Path $zip).Hash.ToLower()
                if ($want -ne $got) { Stop-Install 'The download failed its integrity check, so nothing was installed.' "expected $want, got $got" }
                Write-Ok 'Verified checksum'
                Expand-Archive -Path $zip -DestinationPath (Join-Path $tmp 'unpacked') -Force
                $built = Join-Path $tmp 'unpacked\yurlungr.exe'
                if (-not (Test-Path $built)) { Stop-Install 'The download is not a valid yurlungr archive.' }
                Write-Ok 'Unpacked'
            } else {
                Write-Note 'No prebuilt binary for this release yet, so yurlungr will be built from source.'
                $fromSource = $true
            }
        }

        # ---- or build from source -----------------------------------------------
        if ($fromSource) {
            if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
                Stop-Install 'Rust is needed to build yurlungr, and it is not installed.' 'Install it from https://rustup.rs (it also needs the "Desktop development with C++" build tools), or wait for a prebuilt release.'
            }
            $root = Join-Path $tmp 'cargo'
            # Start-Process does not quote arguments, and temp folders often have spaces in them.
            $cargoArgs = @('install', '--git', $gitUrl, '--locked', '--root', ('"{0}"' -f $root))
            if ($version -ne 'latest') { $cargoArgs += @('--tag', $version) }
            $cargoArgs += 'yurlungr'
            $out = Join-Path $tmp 'cargo.out'
            $err = Join-Path $tmp 'cargo.err'
            $proc = Start-Process -FilePath 'cargo' -ArgumentList $cargoArgs -NoNewWindow -PassThru -RedirectStandardOutput $out -RedirectStandardError $err
            $i = 0
            while (-not $proc.HasExited) {
                Show-Spinner $i 'Compiling yurlungr from source (this takes a few minutes)'
                $i++
                Start-Sleep -Milliseconds 100
            }
            $proc.WaitForExit()   # without this ExitCode can still be empty
            if ($proc.ExitCode -ne 0) {
                Write-Bad 'The build failed.'
                Get-Content $err -Tail 15 -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "      $_" }
                throw 'yurlungr-install-aborted'
            }
            Write-Ok 'Compiled yurlungr'
            $built = Join-Path $root 'bin\yurlungr.exe'
        }

        # ---- put it in place -----------------------------------------------------
        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
        if (Test-Path $exe) {
            # A running session locks its executable, but Windows lets us rename it aside.
            Remove-Item "$exe.old" -Force -ErrorAction SilentlyContinue
            Move-Item -Path $exe -Destination "$exe.old" -Force
        }
        Copy-Item -Path $built -Destination $exe -Force
        Remove-Item "$exe.old" -Force -ErrorAction SilentlyContinue
        Write-Ok "Installed to $exe"

        # ---- PATH -----------------------------------------------------------------
        $pathReady = $true
        if ($modifyPath) {
            if (Add-ToUserPath $installDir) {
                Send-EnvironmentChange
                Write-Ok "Added $installDir to your PATH"
                $pathReady = $false
            }
        } elseif (-not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -ieq $installDir.TrimEnd('\') })) {
            Write-Note "$installDir is not on your PATH (left alone, as asked)."
            $pathReady = $false
        }
        # This session too, so `yurlungr` works right away when run from PowerShell.
        if (-not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -ieq $installDir.TrimEnd('\') })) {
            $env:Path = "$installDir;$env:Path"
        }

        # ---- check it runs ----------------------------------------------------------
        $versionText = (& $exe --version 2>&1 | Out-String).Trim()
        if ($LASTEXITCODE -ne 0 -or -not $versionText) { Stop-Install 'The installed program does not run.' $versionText }
        Write-Ok "Works: $versionText"

        Write-Host ''
        Write-Host "  $versionText is installed."
        Write-Host ''
        Write-Host '    Start it          yurlungr'
        Write-Host '    Agent hooks       yurlungr setup'
        Write-Host '    Uninstall         $env:YURLUNGR_UNINSTALL=1; irm https://raw.githubusercontent.com/' -NoNewline
        Write-Host "$repo/main/install.ps1 | iex"
        Write-Host "    Docs              https://github.com/$repo"
        Write-Host ''
        if ($modifyPath -and -not $pathReady) {
            Write-Host '  Type yurlungr now in this PowerShell window, or open a new terminal window'
            Write-Host '  (Command Prompt, Windows Terminal, ...) and type it there.'
        } else {
            Write-Host '  Type yurlungr in any terminal to begin.'
        }
        Write-Host ''
        if ($upgrading) {
            Write-Host '  A session that is already running keeps the old version until you run: yurlungr kill'
            Write-Host ''
        }
    } catch {
        if ($_.Exception.Message -ne 'yurlungr-install-aborted') {
            Write-Bad "Something went wrong: $($_.Exception.Message)"
        }
        $global:LASTEXITCODE = 1
    } finally {
        Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Install-Yurlungr
