<?php
// Every way a connection can fail to happen, with the errno and the text
// php puts in the out-parameters — which are not the same for a transport
// nobody registered (errno 1 from the client, 0 from the server) as for an
// address that never parsed (errno 0, and no system call attempted).

foreach ([
    'tcp://127.0.0.1',            // no port: not a default, a parse error
    'bogus://x',                  // no such transport
    'tcp://127.0.0.1:1',          // nothing listening
    'unix:///no/such/socket',     // no such path
] as $address) {
    $r = @stream_socket_client($address, $errno, $errstr, 2);
    printf("%-26s %-6s errno=%-3d %s\n", $address, var_export($r, true), $errno, $errstr);
}

// The same failures, unsuppressed: php warns and names the address.
$r = stream_socket_client('tcp://127.0.0.1:1', $e, $s);
$r = stream_socket_client('bogus://x', $e, $s);
$r = stream_socket_server('bogus://x', $e, $s);
$r = fsockopen('127.0.0.1', 1, $e, $s, 2);

// An accept that nothing is coming to times out; a stream that is not a
// socket at all has no errno to report and php shows its strerror(0).
$srv = stream_socket_server('tcp://127.0.0.1:0', $e, $s);
var_dump(stream_socket_accept($srv, 0));
$mem = fopen('php://memory', 'r+');
var_dump(stream_socket_accept($mem, 0));

// The socket calls on a stream that has no socket under it.
var_dump(stream_socket_get_name($mem, false));
var_dump(stream_socket_sendto($mem, 'x'));
var_dump(stream_set_timeout($mem, 1));
var_dump(stream_socket_shutdown($mem, STREAM_SHUT_RDWR));
fclose($mem);
fclose($srv);
