<?php
// The http:// wrapper, against a server this script starts itself.
//
// The server is a second copy of the *running* interpreter (PHP_BINARY), so
// php tests php against php and rphp tests rphp against rphp, with no port
// and no path in the output. The child prints the port it bound before it
// accepts anything, which is what keeps the two sides in step — there is no
// sleeping and no retrying here.

$server = <<<'PHP'
<?php
$srv = stream_socket_server('tcp://127.0.0.1:0', $e, $s);
echo explode(':', stream_socket_get_name($srv, false))[1], "\n";
flush();
while (true) {
    $c = stream_socket_accept($srv, 10);
    if ($c === false) { break; }
    // The request head, then whatever Content-Length promised.
    $req = '';
    while (!str_contains($req, "\r\n\r\n")) {
        $b = fread($c, 1);
        if ($b === '' || $b === false) { break 2; }
        $req .= $b;
    }
    $body = '';
    if (preg_match('/^Content-Length:\s*(\d+)/mi', $req, $m) && (int) $m[1] > 0) {
        while (strlen($body) < (int) $m[1]) {
            $chunk = fread($c, (int) $m[1] - strlen($body));
            if ($chunk === '' || $chunk === false) { break; }
            $body .= $chunk;
        }
    }
    $path = explode(' ', $req)[1] ?? '/';
    // Fixed responses: no Date, nothing that differs between two runs.
    if ($path === '/quit') { fclose($c); break; }
    if ($path === '/404') {
        $out = "HTTP/1.1 404 Not Found\r\nX-Made-Up: yes\r\nContent-Length: 7\r\nConnection: close\r\n\r\nmissing";
    } elseif ($path === '/redir') {
        $out = "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 5\r\nConnection: close\r\n\r\nmoved";
    } elseif ($path === '/final') {
        $out = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nFINAL";
    } elseif (preg_match('#^/r(\d+)$#', $path, $rc)) {
        $out = "HTTP/1.1 {$rc[1]} Redirect\r\nLocation: /echo\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        fwrite($c, $out);
        fclose($c);
        continue;
    } elseif ($path === '/echo') {
        $method = explode(' ', $req)[0];
        preg_match('/^X-Test:\s*(.*)$/mi', $req, $t);
        preg_match('/^Content-Type:\s*(.*)$/mi', $req, $ct);
        $seen = "method=$method xtest=" . trim($t[1] ?? '-')
              . " ct=" . trim($ct[1] ?? '-') . " body=$body";
        $out = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: " . strlen($seen)
             . "\r\nConnection: close\r\n\r\n$seen";
    } else {
        $out = "HTTP/1.1 200 OK\r\nX-Greeting: hello\r\nContent-Length: 4\r\nConnection: close\r\n\r\nROOT";
    }
    fwrite($c, $out);
    fclose($c);
}
PHP;

$http_response_header = null;
file_put_contents('server.php', $server);
$proc = proc_open(
    [PHP_BINARY, '-n', 'server.php'],
    [1 => ['pipe', 'w'], 2 => ['pipe', 'w']],
    $pipes
);
// Blocks until the child has bound: no race, no sleep.
$port = trim(fgets($pipes[1]));
$B = "http://127.0.0.1:$port";

echo "--- get ---\n";
var_dump(file_get_contents("$B/"));
$h = http_get_last_response_headers();
var_dump($h[0], in_array('X-Greeting: hello', $h, true));

// The same lines reach the old predefined variable. php 8.5's *compiler*
// deprecates reading it in a scope that never writes it, so the write below
// is what keeps this snippet quiet on both engines — and the wrapper still
// overwrites it, which is the point being checked.
var_dump($http_response_header === $h);

echo "--- fopen and its metadata ---\n";
$fh = fopen("$B/", 'r');
$m = stream_get_meta_data($fh);
// The keys php reports for a wrapper-opened stream, in php's order.
var_dump(array_keys($m));
var_dump($m['wrapper_type'], $m['stream_type'], $m['mode'], $m['seekable'], $m['unread_bytes']);
var_dump($m['uri'] === "$B/");
// The body arrives from the live socket, in whatever pieces are asked for.
var_dump(fread($fh, 2), fread($fh, 2), feof($fh));
fclose($fh);

echo "--- the request the wrapper builds ---\n";
$ctx = stream_context_create(['http' => [
    'method' => 'POST',
    'header' => "X-Test: abc",
    'content' => 'the-body',
]]);
var_dump(file_get_contents("$B/echo", false, $ctx));

echo "--- redirects ---\n";
var_dump(file_get_contents("$B/redir"));
// Every hop's headers accumulate, first response first.
$h = http_get_last_response_headers();
var_dump($h[0], in_array('HTTP/1.1 200 OK', $h, true));
$no = stream_context_create(['http' => ['follow_location' => 0]]);
var_dump(file_get_contents("$B/redir", false, $no));

// 301, 302 and 303 are followed as a GET with the body — and the headers
// that described it — dropped; 307 and 308 repeat the request unchanged.
foreach ([301, 302, 303, 307, 308] as $code) {
    printf("%d %s\n", $code, file_get_contents("$B/r$code", false, $ctx));
}

echo "--- a status php will not open ---\n";
// The warning quotes the url, so the ephemeral port is masked out of it
// rather than kept out of the snippet.
var_dump(@file_get_contents("$B/404"));
var_dump(str_replace(":$port", ':<port>', error_get_last()['message']));
$ignore = stream_context_create(['http' => ['ignore_errors' => true]]);
var_dump(file_get_contents("$B/404", false, $ignore));

echo "--- get_headers ---\n";
var_dump(get_headers("$B/"));
$assoc = get_headers("$B/404", true);
var_dump($assoc[0], $assoc['X-Made-Up']);

echo "--- file() and readfile() ---\n";
var_dump(file("$B/"));
var_dump(readfile("$B/"));
echo "\n";

echo "--- nothing listening ---\n";
var_dump(file_get_contents('http://127.0.0.1:1/'));

@file_get_contents("$B/quit");
fclose($pipes[1]);
fclose($pipes[2]);
proc_close($proc);
@unlink('server.php');
