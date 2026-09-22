<?php
// A tcp server and a client in one process, over loopback on an ephemeral
// port. Nothing prints the port or the host's own names, so the snippet is
// the same everywhere it runs.

$srv = stream_socket_server('tcp://127.0.0.1:0', $errno, $errstr);
var_dump($srv !== false, $errno, $errstr, get_resource_type($srv));

// The bound name is the address the kernel chose; only its shape is fixed.
[$host, $port] = explode(':', stream_socket_get_name($srv, false));
var_dump($host, $port > 0);

$cli = stream_socket_client("tcp://127.0.0.1:$port", $cerrno, $cerrstr, 5);
var_dump($cli !== false, $cerrno, $cerrstr);
$conn = stream_socket_accept($srv, 5, $peer);
var_dump($conn !== false, explode(':', $peer)[0]);

// A socket is an ordinary stream: the whole read/write surface works on it.
var_dump(fwrite($cli, "ping\n"));
var_dump(fgets($conn));
var_dump(fwrite($conn, "pong\n"));
var_dump(fread($cli, 5));
var_dump(fwrite($cli, "one\ntwo\n"));
var_dump(stream_get_line($conn, 100, "\n"));
var_dump(stream_get_line($conn, 100, "\n"));

// Metadata: a socket has no wrapper_type, is never seekable, and names its
// transport rather than its wrapper.
$m = stream_get_meta_data($cli);
unset($m['uri']);
var_dump($m);

// Both ends agree about who is who.
var_dump(stream_socket_get_name($cli, true) === "127.0.0.1:$port");
var_dump(stream_socket_get_name($conn, false) === "127.0.0.1:$port");

// A half-close is visible as end-of-stream on the other side, and feof()
// says so without waiting for a read to come back short.
var_dump(stream_socket_shutdown($cli, STREAM_SHUT_WR));
var_dump(fgets($conn));
var_dump(feof($conn));

fclose($conn);
fclose($cli);
fclose($srv);
