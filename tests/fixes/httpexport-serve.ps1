# Serves a file over HTTP on 127.0.0.1 for the httpexport test, until 10 s pass without a request.
param([string]$path, [int]$port)
$size = (Get-Item $path).Length
$listener = New-Object Net.Sockets.TcpListener([Net.IPAddress]::Loopback, $port)
$listener.Start()
$buf = New-Object byte[] 65536
$idle = (Get-Date).AddSeconds(10)
while ((Get-Date) -lt $idle) {
    if (-not $listener.Pending()) { Start-Sleep -Milliseconds 20; continue }
    $client = $listener.AcceptTcpClient()
    $net = $client.GetStream()
    $null = $net.Read($buf, 0, $buf.Length)
    $file = $null
    try {
        $head = [Text.Encoding]::ASCII.GetBytes("HTTP/1.1 200 OK`r`nContent-Length: $size`r`nConnection: close`r`n`r`n")
        $net.Write($head, 0, $head.Length)
        $file = [IO.File]::OpenRead($path)
        while (($n = $file.Read($buf, 0, $buf.Length)) -gt 0) { $net.Write($buf, 0, $n) }
    } catch {
    } finally {
        if ($file) { $file.Close() }
        $client.Close()
    }
    $idle = (Get-Date).AddSeconds(10)
}
$listener.Stop()
