# Serves a file over HTTP on 127.0.0.1 for the httpexport test, until 60 s pass without a request. Creates
# <file>.ready once it listens.
param([string]$path, [int]$port)
$size = (Get-Item $path).Length
$listener = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, $port)
$listener.Start()
New-Item -ItemType File "$path.ready" -Force | Out-Null
$buf = New-Object byte[] 65536
$idle = (Get-Date).AddSeconds(60)
while ((Get-Date) -lt $idle) {
    if (-not $listener.Pending()) { Start-Sleep -Milliseconds 20; continue }
    $client = $listener.AcceptTcpClient()
    $client.ReceiveTimeout = 10000
    $net = $client.GetStream()
    $file = $null
    try {
        # The whole request: bytes left unread when the socket closes make the close a reset, which can drop the
        # end of the response.
        $request = ""
        while (-not $request.Contains("`r`n`r`n")) {
            $n = $net.Read($buf, 0, $buf.Length)
            if ($n -le 0) { break }
            $request += [Text.Encoding]::ASCII.GetString($buf, 0, $n)
        }
        $head = [Text.Encoding]::ASCII.GetBytes("HTTP/1.1 200 OK`r`nContent-Length: $size`r`nConnection: close`r`n`r`n")
        $net.Write($head, 0, $head.Length)
        # Delete sharing: a transfer still running (DreamDaemon killed mid-download) mustn't stop the test's cleanup.
        $file = [IO.File]::Open($path, 'Open', 'Read', 'Read, Delete')
        while (($n = $file.Read($buf, 0, $buf.Length)) -gt 0) { $net.Write($buf, 0, $n) }
    } catch {
    } finally {
        if ($file) { $file.Close() }
        $client.Close()
    }
    $idle = (Get-Date).AddSeconds(60)
}
$listener.Stop()
